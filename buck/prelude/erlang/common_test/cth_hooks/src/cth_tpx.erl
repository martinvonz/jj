%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format
-module(cth_tpx).

%% Callbacks
-export([id/1]).
-export([init/2]).

-export([pre_init_per_suite/3]).
-export([post_init_per_suite/4]).
-export([pre_end_per_suite/3]).
-export([post_end_per_suite/4]).

-export([pre_init_per_group/4]).
-export([post_init_per_group/5]).
-export([pre_end_per_group/4]).
-export([post_end_per_group/5]).

-export([pre_init_per_testcase/4]).
-export([post_init_per_testcase/5]).
-export([pre_end_per_testcase/4]).
-export([post_end_per_testcase/5]).

-export([on_tc_fail/4]).
-export([on_tc_skip/4]).

-export([terminate/1]).

%% For tests purposes

-include("method_ids.hrl").

%% ----------------------- Types --------------------------

%  `SUCCESS`, `FAILURE`, `ASSUMPTION_VIOLATION`, `DISABLED`, `EXCLUDED`, `DRY_RUN`

%% -----------------------------------------------------------------------------
%%            Types
%% -----------------------------------------------------------------------------

-type tree_node() :: cth_tpx_test_tree:tree_node().

-record(state, {
    io_buffer :: pid() | undefined,
    suite :: string(),
    groups :: list(string()),
    starting_times :: starting_times(),
    tree_results :: tree_node(),
    previous_group_failed :: string(),
    output :: {file, string()} | stdout
}).

-type hook_opts() :: #{role := top, result_json => string()} | #{role := bot}.

-type shared_state() :: #state{}.
-type hook_state() :: #{
    id := term(),
    role := ct_tpx_role:role(),
    server := shared_state()
}.
-type starting_times() :: #{method_id() => float()}.

%% -----------------------------------------------------------------------------

-spec second_timestamp() -> float().
second_timestamp() ->
    os:system_time(millisecond) / 1000.


%% -----------------------------------------------------------------------------
%%    Registering and collecting results.
%% -----------------------------------------------------------------------------

% General workflow:
% ct will call methods pre_ post_ method before each method init, case, end methods from
% the test_suite.
% Based on the state in each of these, we create a result that will be passed to the method
% add_result/4.
% This one will register the results into a tree, using the method cth_tpx_test_tree:register_result/4.
% Once the whole run is finished, the method terminate/1 is called.
% This one will, for each requested_test creates and output a method_result, using the
% previously constructed tree_result.


%%%%%%%%%%%%%%%%%% This part is similar to the one in cth_tespilot (execpt for some minor modifications
%% in representing init / main/ end testcases as {Phase, Name})

%% -----------------------------------------------------------------------------
%% Format Functions
%% -----------------------------------------------------------------------------

fmt_skip(Suite, CasePat, CaseArgs, Reason) ->
    fmt_stack(Suite, CasePat, CaseArgs, Reason, "SKIPPED").

fmt_fail(Suite, CasePat, CaseArgs, Reason) ->
    fmt_stack(Suite, CasePat, CaseArgs, Reason, "FAILED").

fmt_stack(Suite, CasePat, CaseArgs, {_Outcome, {_Suite, end_per_testcase, {'EXIT', {Reason, ST}}}}, Label) ->
    fmt_stack(Suite, CasePat, CaseArgs, {Reason, ST}, Label);
fmt_stack(Suite, CasePat, CaseArgs, {_Class, {Reason, ST}}, Label) ->
    fmt_stack(Suite, CasePat, CaseArgs, {Reason, ST}, Label);
fmt_stack(_Suite, _CasePat, _CaseArgs, Reason, _Label) ->
    Output = ct_error_printer:format_error(Reason, true),
    unicode:characters_to_list(io_lib:format("~s", [Output])).

%% -----------------------------------------------------------------------------
%% CT hooks functions
%% -----------------------------------------------------------------------------

%% @doc Return a unique id for this CTH.
-spec id(hook_opts()) -> term().
id(#{role := Role}) ->
    {?MODULE, Role}.

%% @doc Always called before any other callback function. Use this to initiate
%% any common state.
-spec init(_Id :: term(), Opts :: hook_opts()) -> {ok, hook_state()}.
init(Id, Opts = #{role := Role}) ->
    ServerName = '$cth_tpx$server$',
    case Role of
        top ->
            Output =
                case maps:get(result_json, Opts, undefined) of
                    undefined -> stdout;
                    FN -> {file, FN}
                end,
            init_role_top(Id, ServerName, Output);
        bot ->
            init_role_bot(Id, ServerName)
    end.

-spec init_role_top(Id :: term(), ServerName :: atom(), Output :: stdout | {file, string()}) -> {ok, hook_state()}.
init_role_top(Id, ServerName, Output) ->
    % IoBuffer that will catpures all the output produced by ct
    IoBuffer = whereis(cth_tpx_io_buffer),
    case IoBuffer of
        undefined ->
            undefined;
        Pid ->
            unregister(user),
            unregister(cth_tpx_io_buffer),
            register(user, Pid)
    end,
    SharedState = #state{
        output = Output,
        starting_times = #{},
        io_buffer = IoBuffer,
        groups = []
    },
    Handle = cth_tpx_server:start_link(SharedState),
    % Register so that init_role_bot can find it
    register(ServerName, Handle),
    HookState = #{
        id => Id,
        role => top,
        server => Handle
    },
    {ok, HookState, cth_tpx_role:role_priority(top)}.

-spec init_role_bot(Id :: term(), ServerName :: atom()) -> {ok, hook_state()}.
init_role_bot(Id, ServerName) ->
    % Put there by init_role_top
    Handle = whereis(ServerName),
    unregister(ServerName),
    HookState = #{
        id => Id,
        role => bot,
        server => Handle
    },
    {ok, HookState, cth_tpx_role:role_priority(bot)}.

%% @doc Called before init_per_suite is called.
-spec pre_init_per_suite(string(), any(), hook_state()) -> hook_state().
pre_init_per_suite(Suite, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        initialize_stdout_capture(State),
        State1 = capture_starting_time(State, ?INIT_PER_SUITE),
        {Config, State1#state{
            suite = Suite,
            groups = [],
            tree_results = cth_tpx_test_tree:new_node(Suite),
            previous_group_failed = false
        }}
    end).

%% @doc Called after init_per_suite.
post_init_per_suite(Suite, _Config, {skip, {failed, _Reason}} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_stack(Suite, "", [], Error, "init_per_suite FAILED"),
        {Error, add_result(?INIT_PER_SUITE, failed, Desc, State)}
    end);
post_init_per_suite(Suite, _Config, {skip, _Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        % In this case the init_per_suite returns with a {skip, Reason}
        % It then passed fine.
        Desc = fmt_stack(Suite, "", [], Error, "init_per_suite SKIPPED"),
        {Error, add_result(?INIT_PER_SUITE, passed, Desc, State)}
    end);
post_init_per_suite(Suite, _Config, {fail, _Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_stack(Suite, "", [], Error, "init_per_suite FAILED"),
        {Error, add_result(?INIT_PER_SUITE, failed, Desc, State)}
    end);
post_init_per_suite(Suite, _Config, Error, HookState) when not is_list(Error) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_stack(Suite, "", [], Error, "init_per_suite FAILED"),
        {Error, add_result(?INIT_PER_SUITE, failed, Desc, State)}
    end);
post_init_per_suite(_Suite, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State) ->
        {Return, add_result(?INIT_PER_SUITE, passed, <<"">>, State)}
    end).

%% @doc Called before end_per_suite.
pre_end_per_suite(_Suite, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        initialize_stdout_capture(State),
        {Config, capture_starting_time(State, ?END_PER_SUITE)}
    end).

%% @doc Called after end_per_suite.
post_end_per_suite(
    Suite,
    _Config,
    {skip, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        Desc = fmt_stack(Suite, "", [], Error, "end_per_suite SKIPPED"),
        State1 = add_result(?END_PER_SUITE, skipped, Desc, State0),
        {Error, clear_suite(State1)}
    end);
post_end_per_suite(
    Suite,
    _Config,
    {fail, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        Desc = fmt_stack(Suite, "", [], Error, "end_per_suite FAILED"),
        State1 = add_result(?END_PER_SUITE, failed, Desc, State0),
        {Error, clear_suite(State1)}
    end);
post_end_per_suite(
    Suite,
    _Config,
    {error, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        Desc = fmt_stack(Suite, "", [], Error, "end_per_suite FAILED"),
        State1 = add_result(?END_PER_SUITE, failed, Desc, State0),
        {Error, clear_suite(State1)}
    end);
post_end_per_suite(_Suite, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0) ->
        %% clean TC state
        State1 = add_result(?END_PER_SUITE, passed, <<"">>, State0),
        {Return, clear_suite(State1)}
    end).

clear_suite(#state{io_buffer = IoBuffer} = State) ->
    case IoBuffer of
        undefined -> ok;
        Pid -> io_buffer:stop_capture(Pid)
    end,
    State#state{
        io_buffer = undefined,
        suite = undefined,
        groups = [],
        starting_times = #{}
    }.

%% @doc Called before each init_per_group.
pre_init_per_group(_SuiteName, _Group, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun
        (State = #state{groups = [_ | Groups], previous_group_failed = true}) ->
            initialize_stdout_capture(State),
            State1 = capture_starting_time(State, ?INIT_PER_GROUP),
            {Config, State1#state{groups = Groups, previous_group_failed = false}};
        (#state{} = State) ->
            initialize_stdout_capture(State),
            {Config, capture_starting_time(State, ?INIT_PER_GROUP)}
    end).

%% @doc Called after each init_per_group.
post_init_per_group(
    _SuiteName,
    Group,
    _Config,
    {skip, {failed, _Reason}} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_stack(Suite, "~s", [Group], Error, "init_per_group FAILED"),
        State2 = add_result(?INIT_PER_GROUP, failed, Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(
    _SuiteName,
    Group,
    _Config,
    {skip, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_stack(Suite, "~s", [Group], Error, "init_per_group SKIPPED"),
        State2 = add_result(?INIT_PER_GROUP, skipped, Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(
    _SuiteName,
    Group,
    _Config,
    {fail, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_stack(Suite, "~s", [Group], Error, "init_per_group FAILED"),
        State2 = add_result(?INIT_PER_GROUP, failed, Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(_SuiteName, Group, _Config, Error, HookState) when not is_list(Error) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_stack(Suite, "~s", [Group], Error, "init_per_group FAILED"),
        State2 = add_result(?INIT_PER_GROUP, failed, Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(_SuiteName, Group, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        State2 = add_result(?INIT_PER_GROUP, passed, <<"">>, State1),
        {Return, ok_group(State2)}
    end).

ok_group(State) ->
    State#state{previous_group_failed = false}.

fail_group(State) ->
    State#state{previous_group_failed = true}.

%% @doc Called after each end_per_group.
pre_end_per_group(_SuiteName, _Group, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        initialize_stdout_capture(State),
        {Config, capture_starting_time(State, ?END_PER_GROUP)}
    end).

%% @doc Called after each end_per_group.
post_end_per_group(
    _SuiteName,
    Group,
    _Config,
    {skip, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        Desc = fmt_stack(Suite, "~s", [Group], Error, "end_per_group SKIPPED"),
        State1 = add_result(?END_PER_GROUP, skipped, Desc, State0),
        {Error, State1#state{groups = tl(Groups)}}
    end);
post_end_per_group(
    _SuiteName,
    Group,
    _Config,
    {fail, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        Desc = fmt_stack(Suite, "~s", [Group], Error, "end_per_group FAILED"),
        State1 = add_result(?END_PER_GROUP, failed, Desc, State0),
        {Error, State1#state{groups = tl(Groups)}}
    end);
post_end_per_group(
    _SuiteName,
    Group,
    _Config,
    {error, _Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        Desc = fmt_stack(Suite, "~s", [Group], Error, "end_per_group FAILED"),
        State1 = add_result(?END_PER_GROUP, failed, Desc, State0),
        {Error, State1#state{groups = tl(Groups)}}
    end);
post_end_per_group(_SuiteName, _Group, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0 = #state{groups = Groups}) ->
        State1 = add_result(?END_PER_GROUP, passed, <<"">>, State0),
        {Return, State1#state{groups = tl(Groups)}}
    end).

%% @doc Called before each test case.
pre_init_per_testcase(_SuiteName, TestCase, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        initialize_stdout_capture(State),
        %% store name and start time for current test case
        %% We capture time twice:
        %%  1) For the init_per_testcase.
        %%  2) For the whole testcase = init + actual_testcase + end
        %% The reason behind is that capturing the timing for the actual_testcase
        %% is not straightforward, as there is no pre/post method for it.
        State1 = capture_starting_time(
            capture_starting_time(State, {TestCase, ?INIT_PER_TESTCASE}), {TestCase, ?MAIN_TESTCASE}
        ),
        {Config, State1}
    end).

post_init_per_testcase(
    _SuiteName,
    TestCase,
    _Config,
    {skip, {failed, _Reason}} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State = #state{suite = Suite}) ->
        %% ct skip because of failed init is reported as error
        TC = io_lib:format("~p.[init_per_testcase]", [TestCase]),
        Desc = fmt_stack(Suite, "~s", [TC], Error, "init_per_testcase FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, failed, Desc, State)}
    end);
post_init_per_testcase(
    _SuiteName, TestCase, _Config, {skip, _Reason} = Error, HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State = #state{suite = Suite}) ->
        %% other skips (user skip) are reported as skips
        TC = io_lib:format("~p.[init_per_testcase]", [TestCase]),
        Desc = fmt_stack(Suite, "~s", [TC], Error, "init_per_testcase SKIPPED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, skipped, Desc, State)}
    end);
post_init_per_testcase(
    _SuiteName, TestCase, _Config, {fail, _Reason} = Error, HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State = #state{suite = Suite}) ->
        %% fails are reported as errors
        TC = io_lib:format("~p.[init_per_testcase]", [TestCase]),
        Desc = fmt_stack(Suite, "~s", [TC], Error, "init_per_testcase FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, failed, Desc, State)}
    end);
post_init_per_testcase(_SuiteName, TestCase, _Config, Error, HookState) when
    not is_list(Error) andalso ok =/= Error
->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State = #state{suite = Suite}) ->
        %% terms are reported as errors except ok (missing in CT doc)
        TC = io_lib:format("~p.[init_per_testcase]", [TestCase]),
        Desc = fmt_stack(Suite, "~s", [TC], Error, "init_per_testcase FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, failed, Desc, State)}
    end);
post_init_per_testcase(_SuiteName, TestCase, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State) ->
        %% everything else is ok
        State1 = add_result({TestCase, ?INIT_PER_TESTCASE}, passed, <<"">>, State),
        {Return, State1}
    end).

%% add test result to state
-spec add_result(method_id(), any(), string(), shared_state()) -> shared_state().
add_result(
    Method,
    Outcome,
    Desc,
    State = #state{
        groups = Groups,
        starting_times = ST0,
        tree_results = TreeResults,
        io_buffer = IoBuffer,
        output = {file, OutputFile}
    }
) ->
    NameMethod =
        case Method of
            {TestCase, Phase} -> io_lib:format("~s.~s", [TestCase, atom_to_list(Phase)]);
            NameMethod0 -> NameMethod0
        end,
    StdOut =
        case IoBuffer of
            undefined ->
                "";
            BufferPid ->
                {Io, Truncated} = io_buffer:flush(BufferPid),
                case Truncated of
                    true ->
                        StdOutLocation =
                            case os:getenv("SANDCASTLE") of
                                true ->
                                    "tab Diagnostics: Artifacts/ct_executor.stdout.txt";
                                _ ->
                                    filename:join(
                                        filename:dirname(OutputFile), "ct_executor.stdout.txt"
                                    )
                            end,
                        Io ++
                            io_lib:format(
                                "\n The std_out has been truncated, see ~s for the full suite std_out.",
                                [
                                    StdOutLocation
                                ]
                            );
                    false ->
                        Io
                end
        end,
    QualifiedName = cth_tpx_test_tree:qualified_name(Groups, NameMethod),
    TS = second_timestamp(),
    Result0 = #{
        name => QualifiedName,
        outcome => Outcome,
        details => unicode:characters_to_list(Desc),
        std_out => StdOut
    },
    Result =
        case ST0 of
            #{Method := StartedTime} ->
                Result0#{
                    startedTime => StartedTime,
                    endedTime => TS
                };
            _ ->
                %% If no test data (skipped test cases/groups/suits)
                %% then started time doesn't exist.
                Result0
        end,
    ST1 = maps:remove(Method, ST0),
    NewTreeResults = cth_tpx_test_tree:register_result(TreeResults, Result, Groups, Method),
    State#state{starting_times = ST1, tree_results = NewTreeResults}.

pre_end_per_testcase(_SuiteName, TC, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        {Config, capture_starting_time(State, {TC, ?END_PER_TESTCASE})}
    end).

%% @doc Called after each test case.
post_end_per_testcase(_SuiteName, TC, _Config, ok = Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0) ->
        State1 = add_result({TC, ?END_PER_TESTCASE}, passed, <<"">>, State0),
        {ok, add_result({TC, ?MAIN_TESTCASE}, passed, <<"">>, State1)}
    end);
post_end_per_testcase(_SuiteName, TC, Config, Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{suite = Suite, groups = Groups}) ->
        NextState =
            case lists:keyfind(tc_status, 1, Config) of
                {tc_status, ok} ->
                    %% Test case passed, but we still ended in an error
                    %% same description as ct output
                    %% first report testcase itself
                    State1 = add_result({TC, ?MAIN_TESTCASE}, passed, <<"">>, State0),
                    %% now report end per failure
                    Desc = fmt_stack(
                        Suite,
                        "~s",
                        [format_path(TC, Groups)],
                        Error,
                        "end_per_testcase FAILED"
                    ),
                    add_result({TC, ?END_PER_TESTCASE}, failed, Desc, State1);
                _ ->
                    %% Test case failed, in which case on_tc_fail already reports it
                    add_result({TC, ?END_PER_TESTCASE}, passed, <<"">>, State0)
            end,
        {Error, NextState}
    end).

%% @doc Called after post_init_per_suite, post_end_per_suite, post_init_per_group,
%% post_end_per_group and post_end_per_testcase if the suite, group or test case failed.
on_tc_fail(_SuiteName, init_per_suite, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, end_per_suite, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {init_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {end_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {TC, _Group}, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State = #state{suite = Suite, groups = Groups}) ->
        Desc = fmt_fail(Suite, "~s", [format_path(TC, Groups)], Reason),
        add_result({TC, ?MAIN_TESTCASE}, failed, Desc, State)
    end);
on_tc_fail(_SuiteName, TC, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State = #state{suite = Suite, groups = Groups}) ->
        Desc = fmt_fail(Suite, "~s", [format_path(TC, Groups)], Reason),
        add_result({TC, ?MAIN_TESTCASE}, failed, Desc, State)
    end).

%% @doc Called when a test case is skipped by either user action
%% or due to an init function failing. (>= 19.3)
on_tc_skip(_SuiteName, init_per_suite, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, end_per_suite, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {init_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {end_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {TC, _Group}, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        handle_on_tc_skip(TC, Reason, State)
    end);
on_tc_skip(_SuiteName, TC, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        handle_on_tc_skip(TC, Reason, State)
    end).

handle_on_tc_skip(TC, {tc_auto_skip, Reason}, State = #state{suite = Suite, groups = Groups}) ->
    Desc = fmt_fail(Suite, "~s", [format_path(TC, Groups)], Reason),
    NewState = add_result({TC, ?MAIN_TESTCASE}, failed, Desc, State),
    NewState#state{suite = Suite};
handle_on_tc_skip(TC, {tc_user_skip, Reason}, State = #state{suite = Suite, groups = Groups}) ->
    Desc = fmt_skip(Suite, "~s", [format_path(TC, Groups)], Reason),
    NewState = add_result({TC, ?MAIN_TESTCASE}, skipped, Desc, State),
    NewState#state{suite = Suite}.

%% @doc Called when the scope of the CTH is done
-spec terminate(hook_state()) -> ok | {error, _Reason}.
terminate(#{role := top, server := Handle}) ->
    #state{output = Output, tree_results = TreeResults} = cth_tpx_server:get(Handle),
    write_output(Output, term_to_binary(TreeResults));
terminate(#{role := bot}) ->
    ok.

-spec write_output({file, string()} | stdout, string()) -> ok.
write_output({file, FN}, JSON) ->
    io:format("Writing result file ~p", [FN]),
    ok = filelib:ensure_dir(FN),
    file:write_file(FN, JSON);
write_output(stdout, JSON) ->
    io:format(user, "~p", [JSON]).

-spec initialize_stdout_capture(shared_state()) -> ok.
initialize_stdout_capture(#state{io_buffer = IoBuffer} = _State) ->
    case IoBuffer of
        undefined ->
            ok;
        Pid when erlang:is_pid(Pid) ->
            io_buffer:stop_capture(Pid),
            io_buffer:flush(Pid),
            io_buffer:start_capture(Pid)
    end.

-spec capture_starting_time(shared_state(), method_id()) -> shared_state().
capture_starting_time(#state{starting_times = ST0} = State, MethodId) ->
    State#state{starting_times = ST0#{MethodId => second_timestamp()}}.

format_path(TC, Groups) ->
    lists:join([atom_to_list(P) || P <- lists:reverse([TC | Groups])], ".").


-spec on_shared_state(hook_state(), Caller, Default, Action) -> {A, hook_state()} when
  Caller :: cth_tpx_role:responsibility(),
  Default :: A,
  Action :: fun((shared_state()) -> {A, shared_state()}).
on_shared_state(HookState = #{role := Role, server := Handle}, Caller, Default, Action) ->
    case cth_tpx_role:is_responsible(Role, Caller) of
        true ->
            A = cth_tpx_server:modify(Handle, Action),
            {A, HookState};
        false ->
            {Default, HookState}
    end.

-spec modify_shared_state(hook_state(), Caller, Action) -> hook_state() when
  Caller :: cth_tpx_role:responsibility(),
  Action :: fun((shared_state()) -> shared_state()).
modify_shared_state(HookState, Caller, Action) ->
    {ok, NewHookState} = on_shared_state(HookState, Caller, _Default=ok, fun(State) ->
        {ok, Action(State)}
    end),
    NewHookState.
