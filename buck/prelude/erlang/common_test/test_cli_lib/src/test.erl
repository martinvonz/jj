%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%%   User-Facing library for quick-iteration testing of Common Test
%%%
%%%     use test:help() for more information
%%% @end
%%% % @format

-module(test).

-include_lib("common/include/tpx_records.hrl").

%% Public API
-export([
    start/0,
    help/0,
    list/0, list/1,
    rerun/1,
    run/0, run/1,
    reset/0
]).

%% init
-export([
    info/0,
    ensure_initialized/0
]).

-type run_spec() :: string() | non_neg_integer() | [#{name := string(), suite := string()}].
-type run_result() :: {non_neg_integer(), non_neg_integer()}.

-spec start() -> ok.
start() ->
    info(),
    ensure_initialized().

-spec info() -> ok.
info() ->
    io:format("~n"),
    io:format(
        "------------------------------------ Test Shell ------------------------------------~n"
    ),
    io:format(
        "Test Shell for interactive testing. Within this shell you can run tests, and update ~n"
    ),
    io:format(
        "the code by recompiling and hot-code loading it with c(module). You can also load ~n"
    ),
    io:format("additional modules with l(module). ~n"),
    io:format(
        "The `test` module provides functionality to list, run tests. Check the available  ~n"
    ),
    io:format("functions (test:help()): ~n~n"),
    help(),
    io:format("~n").

%% @doc Print a description of all available commands.
-spec help() -> ok.
help() ->
    io:format("Buck2 Common Test Runner Shell Interface~n~n"),
    [
        print_help(F, A)
     || {F, A} <- ?MODULE:module_info(exports),
        F =/= module_info andalso F =/= ensure_initialized andalso F =/= start
    ],
    io:format("~n"),
    io:format("For more information, use the built in help, e.g. h(test, help)~n"),
    ok.

-spec print_help(function(), arity()) -> ok.
print_help(Fun, Arity) ->
    #{args := Args, desc := [DescFirst | DescRest]} = command_description(Fun, Arity),
    FunSig = string:pad(
        io_lib:format("~s:~s(~s)", [?MODULE, Fun, lists:join(", ", Args)]), 30, trailing
    ),
    io:format("~s -- ~s~n", [FunSig, DescFirst]),
    Padding = string:pad("", 34),
    [io:format("~s~s~n", [Padding, DescLine]) || DescLine <- DescRest].

-spec command_description(module(), arity()) -> #{args := [string()], desc := string()}.
command_description(help, 0) ->
    #{args => [], desc => ["print help"]};
command_description(info, 0) ->
    #{args => [], desc => ["print info text"]};
command_description(list, 0) ->
    #{args => [], desc => ["list all available tests"]};
command_description(list, 1) ->
    #{
        args => ["RegExOrModule"],
        desc => ["same as list(), but filter tests with RegEx, or a test suite"]
    };
command_description(list, 2) ->
    #{
        args => ["Module", "RegEx"],
        desc => ["same as list(), but filter tests with RegEx, and a test suite"]
    };
command_description(rerun, 1) ->
    #{
        args => ["IdOrRegex"],
        desc =>
            [
                "runs a test with the shortest possible setup path, the test can ",
                "be given as RegEx matching a single test, or the id from listing. ",
                "This command does *not* recompile the test suite or its dependencies"
            ]
    };
command_description(run, 0) ->
    #{args => [], desc => ["run all tests"]};
command_description(run, 1) ->
    #{
        args => ["IdOrRegex"],
        desc =>
            [
                "same as rerun/1 but does compile the targeted suite, and loads",
                "changed modules in the remote node."
            ]
    };
command_description(reset, 0) ->
    #{args => [], desc => ["restarts the test node, enabling a clean test state"]};
command_description(F, A) ->
    error({help_is_missing, {F, A}}).

%% @doc List all available tests
%% @equiv test:list("")
-spec list() -> non_neg_integer().
list() ->
    list("").

%% @doc List all available tests, filters by the given RegEx. Please check
%% [https://www.erlang.org/doc/man/re.html#regexp_syntax] for the supported
%% regular expression syntax. If a module is given as argument, list all
%% tests from that module instead
-spec list(RegExOrModule :: module() | string()) -> non_neg_integer().
list(RegEx) when is_list(RegEx) ->
    ensure_initialized(),
    Tests = ct_daemon:list(RegEx),
    print_tests(Tests).

%% @doc Run a test given by either the test id from the last list() command, or
%% a regex that matches exactly one test. Tests are run with the shortest possible
%% setup. This call does not recompile the test suite and its dependencies, but
%% runs them as is. You can manually recompile code with c(Module).
%% To reset the test state use reset().
-spec rerun(string() | non_neg_integer() | [#{name := string(), suite := string()}]) ->
    run_result().
rerun(Spec) ->
    ensure_initialized(),
    do_plain_test_run(Spec).

%% @doc update code and run all tests
%% @equiv run("")
-spec run() -> ok | error.
run() ->
    run("").

%% @doc Run a test given by either the test id from the last list() command, or
%% a regex that matches exactly one test. Tests are run with the shortest possible
%% setup. This call does recompile the test suite and its dependencies. You can
%% manually recompile code with c(Module). To reset the test state use reset().
-spec run(string() | non_neg_integer()) -> run_result() | error.
run(RegExOrId) ->
    ensure_initialized(),
    case discover(RegExOrId) of
        [] ->
            {0, 0};
        ToRun ->
            Suites = [maps:get(suite, TestMap) || TestMap <- ToRun],
            case shell_buck2_utils:rebuild_modules(Suites) of
                ok ->
                    io:format("Reloading all changed modules... "),
                    Loaded = ct_daemon:load_changed(),
                    io:format("reloaded ~p modules ~P~n", [erlang:length(Loaded), Loaded, 10]),
                    rerun(ToRun);
                Error ->
                    Error
            end
    end.

%% @doc restarts the test node, enabling a clean test state
-spec reset() -> ok | {error, debugger_mode}.
reset() ->
    case is_debug_session() of
        true ->
            io:format(standard_error, "Cannot reset the test node during a debug session!", []);
        false ->
            Type = ct_daemon_node:get_domain_type(),
            NodeName = ct_daemon_node:stop(),
            ct_daemon:start(#{
                type => Type, name => NodeName, cookie => erlang:get_cookie(), options => []
            })
    end.

%% internal
ensure_initialized() ->
    PrintInit = lists:foldl(
        fun(Fun, Acc) -> Fun() orelse Acc end,
        false,
        [
            fun init_utility_apps/0,
            fun init_node/0,
            fun init_group_leader/0
        ]
    ),
    case PrintInit of
        true ->
            io:format(">> initialization done << ~n", []);
        false ->
            ok
    end.

init_utility_apps() ->
    RunningApps = proplists:get_value(running, application:info()),
    case proplists:is_defined(test_cli_lib, RunningApps) of
        true ->
            false;
        false ->
            io:format("starting utility applications...~n", []),
            case application:ensure_all_started(test_cli_lib) of
                {ok, _} ->
                    true;
                Error ->
                    io:format("ERROR: could not start utility applications:~n~p~n", [Error]),
                    io:format("exiting...~n"),
                    erlang:halt(-1)
            end
    end.

init_node() ->
    case ct_daemon:alive() of
        true ->
            false;
        false ->
            io:format("starting test node...~n", []),
            case application:get_env(test_cli_lib, node_config) of
                undefined ->
                    ct_daemon:start();
                {ok, {Type, NodeName, Cookie}} ->
                    ct_daemon:start(#{
                        name => NodeName,
                        type => Type,
                        cookie => Cookie,
                        options => [{multiply_timetraps, infinity} || is_debug_session()]
                    })
            end,
            case is_debug_session() of
                true ->
                    spawn(fun watchdog/0);
                false ->
                    ok
            end,
            true
    end.

watchdog() ->
    Node = ct_daemon_node:get_node(),
    true = erlang:monitor_node(Node, true),
    receive
        {nodedown, Node} ->
            io:format(
                standard_error,
                "The debugging session ended, termiating the test shell...~n",
                []
            ),
            erlang:halt()
    end.

init_group_leader() ->
    %% set the group leader unconditionally, we need to do this since
    %% during init, the group leader is different then the one from the
    %% started shell
    ct_daemon:set_gl(),
    false.

print_tests([]) ->
    io:format("no tests found~n");
print_tests(Tests) ->
    print_tests_impl(lists:reverse(Tests)).

print_tests_impl([]) ->
    ok;
print_tests_impl([{Suite, SuiteTests} | Rest]) ->
    io:format("~s:~n", [Suite]),
    [io:format("\t~b - ~s~n", [Id, Test]) || {Id, Test} <- SuiteTests],
    print_tests_impl(Rest).

-spec is_debug_session() -> boolean().
is_debug_session() ->
    application:get_env(test_cli_lib, debugger_mode, false).

-spec collect_results(#{module => [string()]}) -> #{string => ct_daemon_core:run_result()}.
collect_results(PerSuite) ->
    maps:fold(
        fun(Suite, Tests, Acc) ->
            %% check if we need to reset the test node
            ensure_per_suite_encapsulation(Suite),
            io:format("running ~b test(s) for ~s with output dir ~s~n", [
                erlang:length(Tests), Suite, ct_daemon:output_dir()
            ]),
            %% run all tests for the current SUITE
            maps:merge(
                Acc,
                ct_daemon:run({discovered, [#{suite => Suite, name => Test} || Test <- Tests]})
            )
        end,
        #{},
        PerSuite
    ).

-spec ensure_per_suite_encapsulation(module()) -> ok.
ensure_per_suite_encapsulation(Suite) ->
    case ct_daemon:setup_state() of
        undefined ->
            ok;
        Setup ->
            case lists:reverse(Setup) of
                [Suite | _] ->
                    ok;
                _ ->
                    %% restart node and preserver listing
                    reset(),
                    ok
            end
    end.

-spec discover(string() | non_neg_integer()) -> [#{name := string(), suite := string()}].
discover(RegExOrId) ->
    case ct_daemon:discover(RegExOrId) of
        {error, not_listed_yet} ->
            ct_daemon:list(""),
            discover(RegExOrId);
        {error, Reason} ->
            io:format("cannot run tests ~0p: ~0p~n", [RegExOrId, Reason]),
            [];
        [] ->
            io:format("no tests found for ~0p~n", [RegExOrId]),
            [];
        Tests ->
            Tests
    end.

-spec do_plain_test_run(run_spec()) -> run_result().
do_plain_test_run([#{} | _] = ToRun) ->
    PerSuite = maps:groups_from_list(
        fun(#{suite := Suite}) -> Suite end,
        fun(#{name := Name}) -> Name end,
        ToRun
    ),
    Results = collect_results(PerSuite),
    io:format("~n", []),
    Result =
        {Passed, Total} = maps:fold(
            fun(Name, Result, {Passed, Total}) ->
                ct_daemon_printer:print_result(Name, Result),
                case Result of
                    pass_result -> {Passed + 1, Total + 1};
                    _ -> {Passed, Total + 1}
                end
            end,
            {0, 0},
            Results
        ),
    ct_daemon_printer:print_summary(Total, Passed, Total - Passed),
    Result;
do_plain_test_run(RegExOrId) ->
    case discover(RegExOrId) of
        [] -> {0, 0};
        ToRun -> do_plain_test_run(ToRun)
    end.
