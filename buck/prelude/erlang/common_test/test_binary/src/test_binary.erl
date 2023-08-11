%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

-module(test_binary).

-export([main/1]).
-include_lib("common/include/buck_ct_records.hrl").
-include_lib("common/include/tpx_records.hrl").
-include_lib("kernel/include/logger.hrl").

% in ms, the time we give to init to stop before halting.
-define(INIT_STOP_TIMEOUT, 5000).

main([TestInfoFile, "list", OutputDir]) ->
    test_logger:set_up_logger(OutputDir, test_listing),
    ExitCode =
        try listing(TestInfoFile, OutputDir) of
            _ ->
                ?LOG_DEBUG("Listing done"),
                0
        catch
            Class:Reason:StackTrace ->
                ?LOG_ERROR(erl_error:format_exception(Class, Reason, StackTrace)),
                1
        after
            test_logger:flush()
        end,
    init:stop(ExitCode),
    receive
    after ?INIT_STOP_TIMEOUT ->
        ?LOG_ERROR(
            io_lib:format("~p failed to terminate within ~c millisecond", [
                ?MODULE, ?INIT_STOP_TIMEOUT
            ])
        ),
        erlang:halt(ExitCode)
    end;
main([TestInfoFile, "run", OutputDir | Tests]) ->
    test_logger:set_up_logger(OutputDir, test_runner),
    ExitCode =
        try running(TestInfoFile, OutputDir, Tests) of
            _ ->
                ?LOG_DEBUG("Running done"),
                0
        catch
            Class:Reason:StackTrace ->
                ?LOG_ERROR(erl_error:format_exception(Class, Reason, StackTrace)),
                1
        after
            test_logger:flush()
        end,
    init:stop(ExitCode),
    receive
    after ?INIT_STOP_TIMEOUT ->
        ?LOG_ERROR(
            io_lib:format("~p failed to terminate within ~c millisecond", [
                ?MODULE, ?INIT_STOP_TIMEOUT
            ])
        ),
        erlang:halt(ExitCode)
    end;
main([TestInfoFile]) ->
    %% without test runner support we run all tests and need to create our own test dir
    OutputDir = string:trim(os:cmd("mktemp -d")),
    test_logger:set_up_logger(OutputDir, test_runner, true),
    try list_and_run(TestInfoFile, OutputDir) of
        true ->
            io:format("~nAt least one test didn't pass!~nYou can find the test output directory here: ~s~n", [OutputDir]),
            erlang:halt(1);
        false ->
            erlang:halt(0)
    catch
        Class:Reason:StackTrace ->
            io:format("~s~n", [erl_error:format_exception(Class, Reason, StackTrace)]),
            erlang:halt(1)
    after
        test_logger:flush()
        % ok
    end;
main(Other) ->
    io:format(
        "Wrong arguments, should be called with ~n - TestInfoFile list OutputDir ~n - TestInfoFile run OuptutDir Tests ~n"
    ),
    io:format(
        "Instead, arguments where: ~p~n",
        [Other]
    ),
    erlang:halt(3).

-spec load_test_info(string()) -> #test_info{}.
load_test_info(TestInfoFile) ->
    {ok, [
        #{
            "dependencies" := Dependencies,
            "test_suite" := SuiteName,
            "test_dir" := TestDir,
            "config_files" := ConfigFiles,
            "providers" := Providers,
            "ct_opts" := CtOpts,
            "extra_ct_hooks" := ExtraCtHooks,
            "erl_cmd" := ErlCmd
        }
    ]} = file:consult(TestInfoFile),
    Providers1 = buck_ct_parser:parse_str(Providers),
    CtOpts1 = make_ct_opts(
        buck_ct_parser:parse_str(CtOpts),
        [buck_ct_parser:parse_str(CTH) || CTH <- ExtraCtHooks]
    ),
    #test_info{
        dependencies = [filename:absname(Dep) || Dep <- Dependencies],
        test_suite = filename:join(filename:absname(TestDir), [SuiteName, ".beam"]),
        config_files = lists:map(fun(ConfigFile) -> filename:absname(ConfigFile) end, ConfigFiles),
        providers = Providers1,
        ct_opts = CtOpts1,
        erl_cmd = ErlCmd
    }.

-type ctopt() :: term().
-type cth() :: module() | {module(), term()}.

-spec make_ct_opts([ctopt()], [cth()]) -> [ctopt()].
make_ct_opts(CtOpts, []) -> CtOpts;
make_ct_opts(CtOpts, ExtraCtHooks) -> [{ct_hooks, ExtraCtHooks} | CtOpts].

-spec load_suite(string()) -> [{atom(), string()}].
load_suite(SuitePath) ->
    {module, Module} = code:load_abs(filename:rootname(filename:absname(SuitePath))),
    {Module, filename:absname(SuitePath)}.

-spec get_hooks(#test_info{}) -> [module()].
get_hooks(TestInfo) ->
    Hooks = lists:append(proplists:get_all_values(ct_hooks, TestInfo#test_info.ct_opts)),
    [
        case HookSpec of
            {HookModule, _InitArguments} when is_atom(HookModule) -> HookModule;
            {HookModule, _InitArguments, Priority} when is_atom(HookModule), is_integer(Priority) -> HookModule;
            HookModule when is_atom(HookModule) -> HookModule
        end
     || HookSpec <- Hooks
    ].

-spec listing(string(), string()) -> ok.
listing(TestInfoFile, OutputDir) ->
    TestInfo = load_test_info(TestInfoFile),
    Listing = get_listing(TestInfo, OutputDir),
    listing_interfacer:produce_xml_file(OutputDir, Listing).

-spec running(string(), string(), [string()]) -> ok.
running(TestInfoFile, OutputDir, Tests) ->
    AbsOutputDir = filename:absname(OutputDir),
    TestInfo = load_test_info(TestInfoFile),
    Listing = get_listing(TestInfo, AbsOutputDir),
    test_runner:run_tests(Tests, TestInfo, AbsOutputDir, Listing).

get_listing(TestInfo, OutputDir) ->
    code:add_paths(TestInfo#test_info.dependencies),
    {Suite, _Path} = load_suite(TestInfo#test_info.test_suite),
    InitProviderState = #init_provider_state{output_dir = OutputDir, suite = Suite},
    Providers0 = [
        buck_ct_provider:do_init(Provider, InitProviderState)
     || Provider <- TestInfo#test_info.providers
    ],
    HookModules = get_hooks(TestInfo),
    Providers1 = [buck_ct_provider:do_pre_listing(Provider) || Provider <- Providers0],
    Listing = list_test:list_tests(Suite, HookModules),
    Providers2 = [buck_ct_provider:do_post_listing(Provider) || Provider <- Providers1],
    [buck_ct_provider:do_terminate(Provider) || Provider <- Providers2],
    Listing.

%% rudimantary implementation for running tests with buck2 open-sourced test runner

list_and_run(TestInfoFile, OutputDir) ->
    os:putenv("ERLANG_BUCK_DEBUG_PRINT", "disabled"),
    TestInfo = load_test_info(TestInfoFile),
    Listing = get_listing(TestInfo, OutputDir),
    Tests = listing_to_testnames(Listing),
    running(TestInfoFile, OutputDir, Tests),
    ResultsFile = filename:join(OutputDir, "result_exec.json"),
    print_results(ResultsFile).

-spec listing_to_testnames(#test_spec_test_case{}) -> [string()].
listing_to_testnames(Listing) ->
    [
        binary_to_list(TestCase#test_spec_test_info.name)
     || TestCase <- Listing#test_spec_test_case.testcases
    ].

-spec print_results(file:filename()) -> boolean().
print_results(ResultsFile) ->
    {ok, Data} = file:read_file(ResultsFile),
    Results = jsone:decode(Data),
    {Summary, AnyFailure} = lists:foldl(fun print_individual_results/2, {#{}, false}, Results),
    io:format("~n~10s: ~b~n~n", ["TOTAL", lists:sum(maps:values(Summary))]),
    [
        io:format("~10ts: ~b~n", [json_interfacer:status_name(Result), Amount])
     || {Result, Amount} <- maps:to_list(Summary)
    ],
    AnyFailure.

-spec print_individual_results(map(), Acc) -> Acc when Acc :: {#{non_neg_integer() => non_neg_integer()}, boolean()}.
print_individual_results(Result, {Summary, AnyFailure}) ->
    #{<<"main">> := #{<<"details">> := Details, <<"status">> := Status, <<"std_out">> := StdOut}} = Result,
    NewAnyFailure =
        case json_interfacer:status_name(Status) of
            passed ->
                AnyFailure;
            skipped ->
                print_details(StdOut, Details),
                AnyFailure;
            omitted ->
                AnyFailure;
            _NotPassed ->
                print_details(StdOut, Details),
                true
        end,
    {Summary#{Status => maps:get(Status, Summary, 0) + 1}, NewAnyFailure}.

-spec print_details(string(), string()) -> ok.
print_details(StdOut, Details) ->
    io:format("~ts~n", [StdOut]),
    io:format("~ts~n", [Details]),
    io:format("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -~n"),
    io:format("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -~n").
