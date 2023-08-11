%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

-module(test_runner).

-include_lib("common/include/tpx_records.hrl").
-include_lib("common/include/buck_ct_records.hrl").
-include_lib("kernel/include/logger.hrl").

-export([run_tests/4, mark_success/1, mark_failure/1]).

-export([parse_test_name/2]).

-define(DEFAULT_OUTPUT_FORMAT, json).

-spec run_tests([string()], #test_info{}, string(), [#test_spec_test_case{}]) -> ok.
run_tests(Tests, #test_info{} = TestInfo, OutputDir, Listing) ->
    check_ct_opts(TestInfo#test_info.ct_opts),
    Suite = list_to_atom(filename:basename(TestInfo#test_info.test_suite, ".beam")),
    StructuredTests = lists:map(fun(Test) -> parse_test_name(Test, Suite) end, Tests),
    case StructuredTests of
        [] ->
            throw(no_tests_to_run);
        [_ | _] ->
            OrderedTests = reorder_tests(StructuredTests, Listing),
            execute_test_suite(#test_env{
                output_format = ?DEFAULT_OUTPUT_FORMAT,
                suite = Suite,
                tests = OrderedTests,
                suite_path = TestInfo#test_info.test_suite,
                output_dir = OutputDir,
                dependencies = TestInfo#test_info.dependencies,
                config_files = TestInfo#test_info.config_files,
                providers = TestInfo#test_info.providers,
                ct_opts = TestInfo#test_info.ct_opts,
                erl_cmd = TestInfo#test_info.erl_cmd
            })
    end.

%% @doc Prepare the test spec and run the test.
-spec execute_test_suite(#test_env{}) -> ok.
execute_test_suite(
    #test_env{
        suite = Suite,
        tests = Tests,
        suite_path = SuitePath,
        output_dir = OutputDir,
        ct_opts = CtOpts
    } =
        TestEnv
) ->
    TestSpec = build_test_spec(
        Suite, Tests, filename:absname(filename:dirname(SuitePath)), OutputDir, CtOpts
    ),
    TestSpecFile = filename:join(OutputDir, "test_spec.spec"),
    lists:foreach(
        fun(Spec) -> file:write_file(TestSpecFile, io_lib:format("~tp.~n", [Spec]), [append]) end,
        TestSpec
    ),
    NewTestEnv = TestEnv#test_env{test_spec_file = TestSpecFile, ct_opts = CtOpts},
    try run_test(NewTestEnv) of
        ok -> ok
    catch
        Class:Reason:StackTrace ->
            ErrorMsg = io_lib:format("run_test failed due to ~ts\n", [
                erl_error:format_exception(Class, Reason, StackTrace)
            ]),
            ?LOG_ERROR(ErrorMsg),
            test_run_fail(
                NewTestEnv, ErrorMsg
            )
    end.

-spec run_test(#test_env{}) -> ok.
run_test(
    #test_env{} = TestEnv
) ->
    register(?MODULE, self()),
    application:set_env(test_exec, test_env, TestEnv, [{persistent, true}]),
    case application:ensure_all_started(test_exec, temporary) of
        {ok, _Apps} ->
            Ref = erlang:monitor(process, test_exec_sup, []),
            receive
                {'DOWN', Ref, _Type, Object, Info} ->
                    test_run_fail(
                        TestEnv,
                        unicode:characters_to_list(
                            io_lib:format(
                                "unexpected exception in the buck2 Common Test runner:\n"
                                "                        application test_exec crashed (~p ~p) ~n",
                                [Object, Info]
                            )
                        )
                    );
                {run_succeed, Result} ->
                    test_run_succeed(TestEnv, Result);
                {run_failed, Result} ->
                    test_run_fail(TestEnv, Result)
            after max_timeout(TestEnv) ->
                {Pid, Monitor} = erlang:spawn_monitor(fun() -> application:stop(test_exec) end),
                receive
                    {'DOWN', Monitor, process, Pid, _} -> ok
                after 5000 -> ok
                end,
                ErrorMsg =
                    "\n***************************************************************\n"
                    "* the suite timed out, all tests will be reported as failure. *\n"
                    "***************************************************************\n",
                test_run_timeout(TestEnv, ErrorMsg)
            end;
        {error, Reason} ->
            ErrorMsg = unicode:characters_to_list(
                io_lib:format("TextExec failed to start due to ~p", [Reason])
            ),
            ?LOG_ERROR(ErrorMsg),
            test_run_fail(
                TestEnv, ErrorMsg
            )
    end.

%% @doc Provides xml result as specified by the tpx protocol when test failed to ran.
-spec test_run_fail(#test_env{}, string()) -> ok.
test_run_fail(#test_env{} = TestEnv, Reason) ->
    provide_output_file(
        TestEnv,
        unicode:characters_to_list(io_lib:format("Test failed to ran due to ~s", [Reason])),
        failed
    ).

-spec test_run_timeout(#test_env{}, string()) -> ok.
test_run_timeout(#test_env{} = TestEnv, Reason) ->
    provide_output_file(
        TestEnv, Reason, timeout
    ).

%% @doc Provides xml result as specified by the tpx protocol when test succeed to ran.
-spec test_run_succeed(#test_env{}, string()) -> ok.
test_run_succeed(#test_env{} = TestEnv, Reason) ->
    provide_output_file(TestEnv, Reason, passed).

%% @doc Provides xml result as specified by the tpx protocol.
-spec provide_output_file(#test_env{}, unicode:chardata(), failed | passed | timeout) -> ok.
provide_output_file(
    #test_env{
        output_dir = OutputDir,
        tests = Tests,
        suite = Suite,
        output_format = OutputFormat,
        test_spec_file = TestSpecFile
    } = _TestEnv,
    ResultExec,
    Status
) ->
    LogFile = test_logger:get_log_file(OutputDir, ct_executor),
    Log = trimmed_content_file(LogFile),
    StdOutFile = test_logger:get_std_out(OutputDir, ct_executor),
    StdOut = trimmed_content_file(StdOutFile),
    OutLog = io_lib:format("ct_executor_log: ~s ~nct_executor_stdout: ~s", [Log, StdOut]),
    ResultsFile = filename:join(OutputDir, "result.json"),
    Results =
        case Status of
            failed ->
                collect_results_broken_run(
                    Tests, Suite, "test binary internal crash", ResultExec, OutLog
                );
            Other when Other =:= passed orelse Other =:= timeout ->
                % Here we either pased or timeout.
                case file:read_file(ResultsFile) of
                    {ok, JsonFile} ->
                        TreeResults = binary_to_term(JsonFile),
                        case TreeResults of
                            undefined ->
                                ErrorMsg =
                                    case Status of
                                        passed ->
                                            io_lib:format(
                                                "ct failed to produced results valid file ~p", [
                                                    ResultsFile
                                                ]
                                            );
                                        timeout ->
                                            undefined
                                    end,
                                collect_results_broken_run(
                                    Tests, Suite, ErrorMsg, ResultExec, OutLog
                                );
                            _ ->
                                case Status of
                                    timeout ->
                                        % The ct node crashed after having produced results:
                                        % some post-processing functionalities might be missing.
                                        % We create a .timeout file at the root of the exec dir
                                        % To alert tpx on the situation.
                                        {ok, FileHandle} = file:open(
                                            filename:join(OutputDir, ".timeout"), [write]
                                        ),
                                        io:format(FileHandle, "~p", [Suite]);
                                    _ ->
                                        ok
                                end,
                                collect_results_fine_run(TreeResults, Tests)
                        end;
                    {error, _Reason} ->
                        ErrorMsg =
                            case Status of
                                timeout ->
                                    undefined;
                                _ ->
                                    io_lib:format("ct failed to produced results file ~p", [
                                        ResultsFile
                                    ])
                            end,
                        collect_results_broken_run(Tests, Suite, ErrorMsg, ResultExec, OutLog)
                end
        end,
    {ok, ResultOuptuFile} =
        case OutputFormat of
            xml ->
                junit_interfacer:write_xml_output(OutputDir, Results, Suite, ResultExec, OutLog);
            json ->
                json_interfacer:write_json_output(OutputDir, Results)
        end,
    JsonLogs = execution_logs:create_dir_summary(OutputDir),
    file:write_file(filename:join(OutputDir, "logs.json"), jsone:encode(JsonLogs)),
    test_artifact_directory:prepare(OutputDir).

trimmed_content_file(File) ->
    case file:open(File, [read]) of
        {error, Reason} ->
            io_lib:format("No ~p file found, reason ~p ", [filename:basename(File), Reason]);
        {ok, IoDevice} ->
            case file:pread(IoDevice, {eof, -5000}, 5000) of
                {error, _} ->
                    case file:pread(IoDevice, bof, 5000) of
                        {ok, Data} -> Data;
                        eof -> io_lib:format("nothing to read from ~s", [File])
                    end;
                {ok, EndOfFile} ->
                    EndOfFile ++
                        io_lib:format("~nFile truncated, see ~p for full output", [
                            filename:basename(File)
                        ])
            end
    end.

%% @doc Provide tpx with a result when CT failed to provide results for tests.
-spec collect_results_broken_run([atom()], atom(), string() | undefined, term(), binary()) ->
    [cth_tpx_test_tree:case_result()].

collect_results_broken_run(Tests, _Suite, ErrorMsg, ResultExec, StdOut) ->
    FormattedErrorMsg =
        case ErrorMsg of
            undefined -> "";
            Msg -> io_lib:format("~ts~n", [Msg])
        end,
    lists:map(
        fun(Test) ->
            #{
                ends => [],
                inits => [],
                main => #{
                    name => lists:flatten(
                        io_lib:format("~s.[main_testcase]", [
                            % We need to reverse the list of groups as the method cth_tpx_test_tree:qualified_name expects them
                            % in the reverse order (as it is designed to be called when exploring the tree of results
                            % where we push at each time the group we are in, leading to them being in reverse order).
                            cth_tpx_test_tree:qualified_name(
                                lists:reverse(Test#ct_test.groups), Test#ct_test.test_name
                            )
                        ])
                    ),
                    details =>
                        unicode:characters_to_list(
                            io_lib:format(
                                "~s~s ~n",
                                [FormattedErrorMsg, ResultExec]
                            )
                        ),
                    startedTime => 0.0,
                    endedTime => 0.0,
                    outcome => failed,
                    std_out => StdOut
                }
            }
        end,
        Tests
    ).

%% @doc Provide the results from the tests as specified by tpx protocol, from the json file
%% provided by ct displaying results of all the tests ran.
-spec collect_results_fine_run(cth_tpx_test_tree:tree_node(), [#ct_test{}]) -> [cth_tpx_test_tree:case_result()].
collect_results_fine_run(TreeResults, Tests) ->
    cth_tpx_test_tree:get_result(TreeResults, maps:from_list(get_requested_tests(Tests))).

%% @doc Returns a list of the tests by classifying from the (sequence) of groups they belong.
%% The list is [{[sequence of groups] => [list of tests belonging to this sequence]}].
%% We make sure to respect the group / test insertion order. That is, if the sequence is
%% g1.t1, g2.t2, g1.t2, g1.t3, g2.t2, we produce:
%% [g1.[t1,t2,t3], g2.[t1,t2]]
-spec get_requested_tests([#ct_test{}]) -> [{[atom()], [atom()]}].
get_requested_tests(Tests) ->
    lists:foldl(
        fun(Test, List) ->
            Groups = Test#ct_test.groups,
            add_or_append(List, {Groups, Test#ct_test.test_name})
        end,
        [],
        Tests
    ).

-spec add_or_append(list({K, list(V)}), {K, V}) -> list({K, list(V)}).
add_or_append(List, {Key, Value}) ->
    List0 = lists:map(
        fun
            ({Key0, Value0}) when Key0 =:= Key -> {Key0, lists:append(Value0, [Value])};
            (Other) -> Other
        end,
        List
    ),
    case List0 =:= List of
        true -> lists:append(List0, [{Key, [Value]}]);
        false -> List0
    end.

%% @doc Built the test_spec selecting the requested tests and
%% specifying the result output.
-spec build_test_spec(atom(), [atom()], string(), string(), [term()]) -> [term()].
build_test_spec(Suite, Tests, TestDir, OutputDir, CtOpts) ->
    ListGroupTest = get_requested_tests(Tests),
    SpecTests = lists:map(
        fun
            ({[], TopTests}) ->
                {cases, TestDir, Suite, TopTests};
            ({Groups, GroupTests}) ->
                GroupPath = [[{Group, []} || Group <- Groups]],
                {groups, TestDir, Suite, GroupPath, {cases, GroupTests}}
        end,
        ListGroupTest
    ),
    ResultOutput = filename:join(OutputDir, "result.json"),
    {TpxCtHook, CtOpts1} = getCtHook(CtOpts, ResultOutput),
    LogDir = set_up_log_dir(OutputDir),
    CtOpts2 = add_spec_if_absent(
        {auto_compile, false}, add_spec_if_absent({logdir, LogDir}, CtOpts1)
    ),
    SpecTests ++ [TpxCtHook] ++ CtOpts2.

%% @doc Create a ct_hook for the test spec by plugging together
-spec getCtHook([term()], string()) -> {term(), [term()]}.
getCtHook(CtOpts, ResultOutput) ->
    {NewOpts, Hooks} = addOptsHook(CtOpts, []),
    CthTpxHooks = [
        {cth_tpx, #{role => top, result_json => ResultOutput}},
        {cth_tpx, #{role => bot}}
    ],
    CtHookHandle = {ct_hooks, CthTpxHooks ++ lists:reverse(Hooks)},
    {CtHookHandle, NewOpts}.

-spec addOptsHook([term()], [term()]) -> {term(), [term()]}.
addOptsHook(CtOpts, Hooks) ->
    case lists:keyfind(ct_hooks, 1, CtOpts) of
        false -> {CtOpts, Hooks};
        {ct_hooks, NewHooks} -> addOptsHook(lists:keydelete(ct_hooks, 1, CtOpts), NewHooks ++ Hooks)
    end.

%% @doc Add a spec tuple to the list of ct_options if a tuple defining the property isn't present yet.
-spec add_spec_if_absent({atom(), term()}, [term()]) -> [term()].
add_spec_if_absent({Key, Value}, CtOpts) ->
    case lists:keyfind(Key, 1, CtOpts) of
        false -> [{Key, Value} | CtOpts];
        _ -> CtOpts
    end.

%% @doc Parse the test name, and decompose it into the test, group and suite atoms
-spec parse_test_name(string(), atom()) -> #ct_test{}.
parse_test_name(Test, Suite) ->
    [Groups0, TestName] = string:split(Test, ".", all),
    Groups1 =
        case Groups0 of
            [] -> [];
            _ -> string:split(Groups0, ":", all)
        end,
    Groups = lists:map(fun(GroupStr) -> list_to_atom(GroupStr) end, Groups1),
    #ct_test{
        suite = Suite,
        groups = Groups,
        test_name = list_to_atom(TestName),
        canonical_name = Test
    }.

-spec reorder_tests(list(#ct_test{}), #test_spec_test_case{}) -> list(#ct_test{}).
reorder_tests(Tests, #test_spec_test_case{testcases = TestCases}) ->
    % This is the ordered lists of test from the suite as
    % binary strings.
    MapNameToTests = lists:foldl(
        fun(#ct_test{canonical_name = Name} = Test, Map) -> Map#{list_to_binary(Name) => Test} end,
        maps:new(),
        Tests
    ),
    lists:foldr(
        fun(#test_spec_test_info{name = TestName}, ListOrdered) ->
            case MapNameToTests of
                #{TestName := Test} -> [Test | ListOrdered];
                _ -> ListOrdered
            end
        end,
        [],
        TestCases
    ).

%% @doc LogDir is the directory where ct will log to.
%% Make sure it exists and returns it.
set_up_log_dir(OutputDir) ->
    LogDir = filename:join(OutputDir, "log_dir"),
    filelib:ensure_path(LogDir),
    LogDir.

%% @doc Informs the test runner of a successful test run.
-spec mark_success(unicode:chardata()) -> ok.
mark_success(Result) ->
    ?MODULE ! {run_succeed, Result},
    ok.

%% @doc Informs the test runner of a fataled test run.
-spec mark_failure(unicode:chardata()) -> ok.
mark_failure(Error) ->
    ?MODULE ! {run_failed, Error},
    ok.

%% @doc CtOpts must be tuple as defined here:
%% https://www.erlang.org/doc/apps/common_test/run_test_chapter.html#test-specification-syntax
%% that will be inserted to the test specification.
%% We do not check here that those are valid, but that they do not conflict with those
%% created here by the runner.
-spec check_ct_opts([term()]) -> ok.
check_ct_opts(CtOpts) ->
    ProblematicsOpts = [suites, groups, cases, skip_suites, skip_groups, skip_cases],
    lists:foreach(
        fun(Opt) ->
            case lists:keyfind(Opt, 1, CtOpts) of
                false ->
                    ok;
                _ ->
                    ?LOG_ERROR("Option ~p is not supported by test runner", [Opt]),
                    throw({non_valid_ct_opt, Opt})
            end
        end,
        ProblematicsOpts
    ).

-spec max_timeout(#test_env{}) -> integer().
max_timeout(#test_env{ct_opts = CtOpts}) ->
    case os:getenv("TPX_TIMEOUT_SEC") of
        false ->
            Multiplier = proplists:get_value(multiply_timetraps, CtOpts, 1),
            %% 9 minutes 30 seconds, giving us 30 seconds to crash multiplied by multiply_timetraps
            round(Multiplier * (9 * 60 + 30) * 1000);
        StrTimeout ->
            InputTimeout = list_to_integer(StrTimeout),
            case InputTimeout of
                _ when InputTimeout > 30 -> (InputTimeout - 30) * 1000;
                _ -> error("Please allow at least 30s for the binary to execute")
            end
    end.
