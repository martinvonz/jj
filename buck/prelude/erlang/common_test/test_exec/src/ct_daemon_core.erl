%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Stateless Core functionality for ct_daemon
%%% @end
%%% % @format

-module(ct_daemon_core).

-include_lib("common/include/tpx_records.hrl").
-include_lib("common/include/buck_ct_records.hrl").
-include_lib("kernel/include/logger.hrl").

%% Public API
-export([
    test_suites/0,
    list/0, list/1,
    run_test/3,
    from_qualified/1,
    to_qualified/1, to_qualified/2
]).

-export([
    path_timetrap/1
]).

-type reason() :: term().
-type run_result() :: term().

-type setup_state() :: {[atom()], [fun((proplists:proplist()) -> term())]}.
-type setup() :: #{config => proplists:proplists(), setup_state => setup_state()}.

-export_type([reason/0, run_result/0, setup/0]).

-define(DEFAULT_TIMETRAP, 30 * 60 * 1000).

-type ct_test_result() ::
    term()
    | {skip, reason()}
    | {comment, Comment :: string()}
    | {save_config, SaveConfig :: ct_suite:ct_config()}
    | {skip_and_save, reason(), SaveConfig :: ct_suite:ct_config()}
    | {error, {init_per_testcase, reason()}}
    | {error, reason()}.

-type test_status() ::
    {error, {atom(), reason()}}
    | {error, {atom(), {skip, reason()}}}
    | {error, {atom(), {skip_and_save, reason()}}}
    | pass_result.

%% listing and discovery
-spec test_suites() -> [module()].
test_suites() ->
    {ok, Pattern} = re:compile("_SUITE$"),
    [
        erlang:list_to_atom(Module)
     || {Module, _Filename, _Loaded} <- code:all_available(), re:run(Module, Pattern) =/= nomatch
    ].

-spec list() -> #{module() => [string()]}.
list() ->
    Suites = test_suites(),
    case erlang:length(Suites) =< 10 of
        true ->
            lists:foldl(
                fun
                    (_, Error = {error, _}) ->
                        Error;
                    (Suite, Tests) ->
                        case list(Suite) of
                            Error = {error, _} -> Error;
                            [] -> Tests;
                            Discovered -> Tests#{Suite => Discovered}
                        end
                end,
                #{},
                Suites
            );
        false ->
            {too_many_suites, 10, erlang:length(Suites)}
    end.

-spec to_qualified(#{suite := module(), name := string()}) -> string().
to_qualified(#{suite := Suite, name := Name}) ->
    to_qualified(Suite, Name).

-spec to_qualified(Suite :: module(), Name :: string()) -> string().
to_qualified(Suite, Name) ->
    lists:flatten(io_lib:format("~s - ~s", [Suite, Name])).

-spec from_qualified(string()) -> #{suite := module(), name := string()}.
from_qualified(FullName) ->
    {match, [_, {ModS, ModL}, {NameS, NameL}]} = re:run(FullName, "(.*) - (.*)"),
    #{
        suite => erlang:list_to_atom(string:slice(FullName, ModS, ModL)),
        name => string:slice(FullName, NameS, NameL)
    }.

-spec list(module()) -> [string()].
list(Suite) ->
    case code:which(Suite) of
        non_existing ->
            {error, {could_not_find_module, Suite}};
        _ ->
            #test_spec_test_case{suite = SuiteName, testcases = TestCases} = list_test:list_tests(
                Suite, ct_daemon_hooks:get_hooks()
            ),
            [
                FullTestName
             || FullTestName <-
                    [
                        to_qualified(SuiteName, TestName)
                     || #test_spec_test_info{name = TestName} <- TestCases
                    ]
            ]
    end.

%% running tests
-spec run_test(#ct_test{}, setup(), file:filename_all()) -> {run_result(), setup()}.
run_test(Spec, PreviousSetup, OutputDir) ->
    case do_incremental_setup(PreviousSetup, Spec, OutputDir) of
        {ok, SetupConfig, SetupState} ->
            {RunResult, AfterRunConfig} = do_run_test(SetupConfig, Spec),
            {RunResult, #{setup_state => SetupState, config => AfterRunConfig}};
        {Skip = {skip, _, _}, SetupConfig, SetupState} ->
            {Skip, #{setup_state => SetupState, config => SetupConfig}};
        {{fail, Where, ST}, SetupConfig, SetupState} ->
            %% we map fail to error
            {{error, {setup_failure, {Where, ST}}}, #{setup_state => SetupState, config => SetupConfig}};
        {{error, R}, SetupConfig, SetupState} ->
            {{error, {setup_failure, R}}, #{setup_state => SetupState, config => SetupConfig}}
    end.

do_incremental_setup(undefined, Spec, OutputDir) ->
    do_fresh_setup(Spec, OutputDir);
do_incremental_setup(
    #{config := Config0, setup_state := {SetupPath0, EndStack0}},
    _Spec = #ct_test{suite = Suite, groups = Groups},
    _OutputDir
) ->
    WantPath = [Suite | Groups],
    {CommonPrefix, RemainingSetup} = get_common_prefix(WantPath, lists:reverse(SetupPath0), []),
    {Config1, CommonPrefix, EndStack1} = do_teardown_until(
        CommonPrefix, SetupPath0, EndStack0, Config0
    ),
    do_setup_from(Suite, Config1, CommonPrefix, EndStack1, RemainingSetup).

do_fresh_setup(#ct_test{suite = Suite, groups = Groups}, OutputDir) ->
    InitialConfig = get_fresh_config(Suite, OutputDir),
    RemainingSetup = [Suite | Groups],
    InitAndEnds = build_inits_and_ends(Suite, RemainingSetup, []),
    do_init(InitAndEnds, InitialConfig, {[], []}).

do_setup_from(Suite, Config, SetupPath, EndStack, RemainingSetup) ->
    InitAndEnds = build_inits_and_ends(Suite, RemainingSetup, []),
    %% return from the last config on the init path if applicable
    ConfigIn =
        case EndStack of
            [{_, LastConfig} | _] -> LastConfig;
            _ -> Config
        end,
    do_init(InitAndEnds, ConfigIn, {SetupPath, EndStack}).

do_init([], Config, SetupState) ->
    {ok, Config, SetupState};
do_init([{Id, Init, End} | Rest], ConfigIn, SetupState = {PathStack, EndsStack}) ->
    Timetrap = path_timetrap(lists:reverse([Id | PathStack])),
    case do_part_safe(Id, Init, ConfigIn, Timetrap) of
        Error = {error, _} -> {Error, ConfigIn, SetupState};
        Skip = {skip, _, _} -> {Skip, ConfigIn, SetupState};
        Fail = {fail, _, _} -> {Fail, ConfigIn, SetupState};
        {ok, ConfigOut} -> do_init(Rest, ConfigOut, {[Id | PathStack], [{End, ConfigOut} | EndsStack]})
    end.

get_common_prefix([L | RestL], [R | RestR], Acc) when L =:= R ->
    get_common_prefix(RestL, RestR, [L | Acc]);
get_common_prefix(RemainingSetup, _, Acc) ->
    {Acc, RemainingSetup}.

do_teardown_until(Target, Setup, EndsStack, Config) when Target =:= Setup ->
    {Config, Target, EndsStack};
do_teardown_until(Target, Path = [Id | RemainingSetup], [{End, Config} | RemainingEndsStack], _) ->
    Timetrap = path_timetrap(lists:reverse(Path)),
    NextConfig =
        case do_part_safe(Id, End, Config, Timetrap) of
            GroupResult = {ok, {return_group_result, _Status}} ->
                [GroupResult | lists:keydelete(return_group_result, 1, Config)];
            _ ->
                lists:keydelete(return_group_result, 1, Config)
        end,
    do_teardown_until(Target, RemainingSetup, RemainingEndsStack, NextConfig).

build_inits_and_ends(_Suite, [], Acc) ->
    lists:reverse(Acc);
build_inits_and_ends(Suite, [Suite | RemainingInit], []) ->
    build_inits_and_ends(
        Suite,
        RemainingInit,
        [
            {Suite, wrap_ct_hook(init_per_suite, [Suite], fun Suite:init_per_suite/1),
                wrap_ct_hook(end_per_suite, [Suite], fun Suite:end_per_suite/1)}
        ]
    );
build_inits_and_ends(Suite, [Group | RemainingInit], Acc) ->
    build_inits_and_ends(
        Suite,
        RemainingInit,
        [
            {Group, wrap_ct_hook(init_per_group, [Suite, Group], fun Suite:init_per_group/2),
                wrap_ct_hook(end_per_group, [Suite, Group], fun Suite:end_per_group/2)}
            | Acc
        ]
    ).

do_run_test(SetupConfig, #ct_test{suite = Suite, groups = Groups, test_name = Test}) ->
    Timetrap = path_timetrap([Suite | Groups], Test),
    Path = [Suite | Groups] ++ [Test],
    PartFun =
        fun(Config) ->
            test_part(Config, Suite, Test, Path)
        end,
    case do_part_safe(Test, PartFun, SetupConfig, Timetrap) of
        Error = {error, _} -> {Error, SetupConfig};
        {ok, Result} -> Result
    end.

test_part(Config, Suite, Test, Path) ->
    InitResult =
        case safe_call(wrap_ct_hook(init_per_testcase, Path, fun Suite:init_per_testcase/2), [Config]) of
            {error, not_exported} -> Config;
            {skipped, Reason} -> {error, {skip, init_per_testcase, Reason}};
            {failed, InitErrReason} -> {error, {skip, init_per_testcase, InitErrReason}};
            {error, InitErrReason} -> {error, {skip, init_per_testcase, InitErrReason}};
            InitOutConfig -> InitOutConfig
        end,
    {TestResult, FinalConfig} =
        case InitResult of
            Error = {error, _} ->
                {Error, Config};
            InitConfig ->
                Result = safe_call(fun Suite:Test/1, [InitConfig]),
                AfterRunConfig = config_from_test_result(Result, InitConfig),
                case
                    safe_call(wrap_ct_hook(end_per_testcase, Path, fun Suite:end_per_testcase/2), [
                        AfterRunConfig
                    ])
                of
                    {save_config, AfterEndConfig} ->
                        {Result, AfterEndConfig};
                    E = {Failed, _} when Failed =:= fail orelse Failed =:= failed ->
                        FinalResult =
                            case status_from_test_result(Result, Test) of
                                pass_result -> {error, {end_per_testcase, E}};
                                _ -> Result
                            end,
                        {FinalResult, AfterRunConfig};
                    _ ->
                        {Result, AfterRunConfig}
                end
        end,
    {status_from_test_result(TestResult, Test), FinalConfig}.

wrap_ct_hook(Part, Path, Fun) ->
    ct_daemon_hooks:wrap(Part, Path, Fun).

%% @doc transform exceptions into error tuples
safe_call(F, Args) ->
    try erlang:apply(F, Args) of
        Res -> Res
    catch
        E:R:ST ->
            ct_daemon_hooks:format_ct_error(E, R, ST)
    end.

-spec config_from_test_result(ct_test_result(), ct_suite:ct_config()) -> ct_suite:ct_config().
config_from_test_result({skip_and_save, _Reason, Config}, _) -> Config;
config_from_test_result({save_config, Config}, _) -> Config;
config_from_test_result({fail, FailReason}, Config) -> [{tc_status, {failed, FailReason}} | Config];
config_from_test_result({skip, FailReason}, Config) -> [{tc_status, {skipped, FailReason}} | Config];
config_from_test_result({error, Error}, Config) -> [{tc_status, {failed, Error}} | Config];
config_from_test_result(_, Config) -> Config.

-spec status_from_test_result(ct_test_result(), atom()) -> test_status().
status_from_test_result(InitError = {error, {init_per_testcase, _}}, _) ->
    InitError;
status_from_test_result({error, SkipResult = {skip, _, _}}, _) ->
    SkipResult;
status_from_test_result({skip_and_save, Reason, _SaveConfig}, Test) ->
    {error, {Test, {skip_and_save, Reason}}};
status_from_test_result({error, {Error, Reason, Stacktrace}}, Test) ->
    {error, {Test, {Error, {Reason, Stacktrace}}}};
status_from_test_result({error, R}, Test) ->
    {error, {Test, R}};
status_from_test_result({fail, R}, Test) ->
    {fail, {Test, R}};
status_from_test_result({skip, R}, Test) ->
    {skip, {Test, R}};
status_from_test_result(_R, _) ->
    pass_result.

get_fresh_config(Suite, OutputDir) ->
    {module, Suite} = code:ensure_loaded(Suite),
    SuitePath = code:which(Suite),
    DataDir = filename:join(filename:dirname(SuitePath), io_lib:format("~s_data", [Suite])) ++ "/",
    PrivDir =
        filename:join([
            OutputDir,
            io_lib:format("~s.~s", [Suite, calendar:system_time_to_rfc3339(erlang:system_time(second))]),
            "private_log"
        ]) ++ "/",
    ok = filelib:ensure_path(PrivDir),
    [{priv_dir, PrivDir}, {data_dir, DataDir}].

%% @doc run an init or end or test in an isolated process like CT
do_part_safe(Id, Fun, Config, TimeTrap) ->
    {Pid, ProcRef} = erlang:spawn_monitor(
        fun() ->
            {ParentPid, RspRef} =
                receive
                    M -> M
                end,
            {name, FunName} = erlang:fun_info(Fun, name),
            try Fun(Config) of
                {skipped, Reason} ->
                    ?LOG_DEBUG("got skip for ~p because of: ~p", [Id, Reason]),
                    ParentPid ! {RspRef, {skip, {FunName, Id}, Reason}};
                {failed, Reason} ->
                    ?LOG_DEBUG("got fail for ~p because of: ~p", [Id, Reason]),
                    ParentPid ! {RspRef, {fail, {FunName, Id}, Reason}};
                {skip_and_save, Reason, _} ->
                    ?LOG_DEBUG("got skip for ~p because of: ~p", [Id, Reason]),
                    ParentPid ! {RspRef, {skip, {FunName, Id}, Reason}};
                Res ->
                    ?LOG_DEBUG("got new result: ~p", [Res]),
                    ParentPid ! {RspRef, Res}
            catch
                error:undef ->
                    ParentPid ! {RspRef, Config};
                E:R:ST ->
                    ?LOG_DEBUG("crashed executing part: ~p", [{E, R, ST}]),
                    ParentPid ! {RspRef, {E, R, ST}}
            end
        end
    ),
    ReqRef = erlang:make_ref(),
    Pid ! {self(), ReqRef},
    receive
        {'DOWN', ProcRef, process, _Pid, Info} ->
            {error, {Id, {'ct_daemon_core$sentinel_crash', Info}}};
        {ReqRef, Skip = {skip, _Where, _Reason}} ->
            Skip;
        {ReqRef, Fail = {fail, _Where, _Reason}} ->
            Fail;
        {ReqRef, {_, _, _} = ErrorTuple} ->
            flush_monitor_msg(ProcRef),
            {error, ErrorTuple};
        {ReqRef, Res} ->
            flush_monitor_msg(ProcRef),
            {ok, Res}
    after TimeTrap -> {error, {Id, {timetrap, TimeTrap}}}
    end.

flush_monitor_msg(Ref) ->
    true = erlang:demonitor(Ref),
    receive
        {'DOWN', Ref, _, _} -> ok
    after 0 -> ok
    end.

%% timetraps

path_timetrap([Suite | Groups]) ->
    do_path_timetrap(#{suite => Suite, groups => Groups}, ?DEFAULT_TIMETRAP).

path_timetrap([Suite | Groups], Test) ->
    do_path_timetrap(#{suite => Suite, groups => Groups, test => Test}, ?DEFAULT_TIMETRAP).

do_path_timetrap(#{suite := Suite} = Spec, Current) ->
    TimeTrap0 = suite_timetrap(Suite, Current),
    Timetrap1 =
        case Spec of
            #{groups := Groups} ->
                lists:foldl(
                    fun(Group, Curr) ->
                        group_timetrap(Suite, Group, Curr)
                    end,
                    TimeTrap0,
                    Groups
                );
            _ ->
                TimeTrap0
        end,
    UnscaledTimetrap =
        case Spec of
            #{test := Test} -> test_timetrap(Suite, Test, Timetrap1);
            _ -> TimeTrap0
        end,
    scale_timetrap(UnscaledTimetrap).

suite_timetrap(Suite, Default) ->
    case erlang:function_exported(Suite, suite, 0) of
        false ->
            Default;
        true ->
            Timetrap = proplists:get_value(timetrap, Suite:suite(), Default),
            timetrap_to_ms(Timetrap)
    end.

group_timetrap(Suite, Group, Default) ->
    case erlang:function_exported(Suite, group, 1) of
        false ->
            Default;
        true ->
            Timetrap = proplists:get_value(timetrap, Suite:group(Group), Default),
            timetrap_to_ms(Timetrap)
    end.

test_timetrap(Suite, Test, Default) ->
    case erlang:function_exported(Suite, Test, 0) of
        false ->
            Default;
        true ->
            Timetrap = proplists:get_value(timetrap, Suite:Test(), Default),
            timetrap_to_ms(Timetrap)
    end.

timetrap_to_ms(MS) when is_integer(MS) -> MS;
timetrap_to_ms({seconds, S}) -> S * 1000;
timetrap_to_ms({minutes, S}) -> S * 1000 * 60;
timetrap_to_ms({hours, S}) -> S * 1000 * 60 * 60;
timetrap_to_ms(_) -> ?DEFAULT_TIMETRAP.

scale_timetrap(TimeTrap) ->
    case application:get_env(test_exec, daemon_options, []) of
        Options when is_list(Options) ->
            case proplists:get_value(multiply_timetraps, Options, 1) of
                infinity -> infinity;
                X when is_number(X) -> TimeTrap * X
            end;
        BadOptions ->
            error({bad_options, BadOptions})
    end.
