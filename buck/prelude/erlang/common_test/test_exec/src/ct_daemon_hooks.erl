%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Implementation of hooks functionality
%%% @end
%%% % @format

-module(ct_daemon_hooks).
-compile(warn_missing_spec).

-behaviour(gen_server).

%% API
-export([
    start_monitor/0,
    set_state/2,
    get_state/1,
    wrap/3,
    format_ct_error/3,
    get_hooks/0
]).

%% gen_server callbacks
-export([
    init/1,
    handle_call/3,
    handle_cast/2
]).

-type id() :: term().
-type opts() :: term().
-type hook_state() :: term().
-type group_name() :: atom().
-type func_name() :: atom().
-type test_name() ::
    init_per_suite
    | end_per_suite
    | {init_per_group, group_name()}
    | {end_per_group, group_name()}
    | {func_name(), group_name()}
    | func_name().
-type config() :: [{atom(), term()}].

-type hook() :: {atom(), id()}.
-type state() :: #{
    hooks := [hook()],
    states := #{id() => hook_state()}
}.

-type part() ::
    init_per_suite
    | init_per_group
    | init_per_testcase
    | end_per_suite
    | end_per_group
    | end_per_testcase
    | on_tc_fail
    | on_tc_skip.

%%--------------------------------------------------------------------
%%% API

-spec set_state(id(), hook_state()) -> ok.
set_state(Id, State) ->
    gen_server:call(?MODULE, {set_state, Id, State}).

-spec get_state(id()) -> {ok, hook_state()} | {error, not_found}.
get_state(Id) ->
    gen_server:call(?MODULE, {get_state, Id}).

-spec wrap(part(), [atom()], fun()) -> fun().
wrap(Part, Path, Fun) ->
    WrappedFun = gen_server:call(?MODULE, {wrap, Part, Fun}),
    %% apply path as closure
    fun(Config) -> WrappedFun(Path, Config) end.

-spec get_hooks() -> [module()].
get_hooks() ->
    [get_hook_module(Hook) || Hook <- get_hooks_config()].

%% @doc
%% Starts the server within supervision tree
-spec start_monitor() -> gen_server:start_ret().
start_monitor() ->
    gen_server:start_monitor({local, ?MODULE}, ?MODULE, [], []).

%%--------------------------------------------------------------------
%%% gen_server callbacks

-spec init([]) -> {ok, state()}.
init([]) ->
    {ok, initialize_hooks()}.

-spec handle_call(Request :: term(), From :: gen_server:from(), State :: state()) ->
    no_return().
handle_call({get_state, Id}, _From, State = #{states := HookStates}) ->
    case HookStates of
        #{Id := HookState} -> {reply, {ok, HookState}, State};
        _ -> {error, not_found, [{state, State}, {id, Id}]}
    end;
handle_call({set_state, Id, HookState}, _From, State = #{states := HookStates}) ->
    {reply, ok, State#{states => HookStates#{Id => HookState}}};
handle_call({wrap, Part, Fun}, _From, State) ->
    {reply, wrap_part(Part, Fun, State), State}.

-spec handle_cast(Request :: term(), State :: state()) -> no_return().
handle_cast(_Request, _State) ->
    error(badarg).

%%--------------------------------------------------------------------
%%% Internal functions

-spec initialize_hooks() -> state().
initialize_hooks() ->
    ConfiguredHooks = get_hooks_config(),
    NormalizedConfiguredHooks = [{get_hook_module(Hook), get_hook_opts(Hook)} || Hook <- ConfiguredHooks],
    %% first we need the Id
    HooksWithId = [{wrapped_id(Mod, Opts), Mod, Opts} || {Mod, Opts} <- NormalizedConfiguredHooks],
    %% according to documentation, if two hooks have the same ID, the latter one get's dropped
    PreInitHooks = lists:ukeysort(1, HooksWithId),
    %% now let's run the inits in order and build the state
    {States, HooksWithPriority} = lists:foldl(
        fun({Id, Mod, Opts}, {StatesAcc, HooksAcc}) ->
            {Priority, HookState} = wrapped_init({Mod, Id}, Opts),
            {StatesAcc#{Id => HookState}, [{Priority, {Mod, Id}} | HooksAcc]}
        end,
        {#{}, []},
        PreInitHooks
    ),

    %% sort hooks according to priority
    %% Note: This is reverse order for the priorities, but since we wrap, we want to wrap the
    %%       lowest priority hook first.
    SortedHooks = lists:keysort(1, HooksWithPriority),

    #{
        states => States,
        hooks => [Hook || {_Priority, Hook} <- SortedHooks]
    }.

get_hooks_config() ->
    application:get_env(test_exec, ct_daemon_hooks, []) ++
        proplists:get_value(ct_hooks, application:get_env(test_exec, daemon_options, []), []).

-spec wrap_part(part(), fun(), state()) -> fun(([atom() | config()]) -> term()).
wrap_part(Part, Fun, State) ->
    wrap_init_end(Part, Fun, State).

wrap_init_end(Part, Fun, #{hooks := Hooks}) ->
    WrappedWithPreAndPost = lists:foldl(
        fun(Hook, FunToWrap) ->
            fun(FullPathArg, ConfigArg0) ->
                PathArg =
                    case level(Part) of
                        testcase ->
                            [Suite | _] = FullPathArg,
                            [Suite, lists:last(FullPathArg)];
                        _ ->
                            FullPathArg
                    end,
                case call_if_exists_with_fallback_store_state(Hook, pre(Part), PathArg ++ [ConfigArg0], ok) of
                    {skip, SkipReason} ->
                        {skipped, SkipReason};
                    {fail, FailReason} ->
                        {failed, FailReason};
                    HookCallbackResult ->
                        ConfigArg1 =
                            case is_list(HookCallbackResult) of
                                true ->
                                    HookCallbackResult;
                                false ->
                                    %% NB. If pre(Part) is not defined in the hook we get 'ok'
                                    ConfigArg0
                            end,
                        %% first step of error handling for the post functions where we set tc_status
                        {PostConfig, Return} =
                            try FunToWrap(PathArg, ConfigArg1) of
                                {skip, SkipReason} ->
                                    {
                                        [
                                            {tc_status, {skipped, SkipReason}}
                                            | lists:keydelete(tc_status, 1, ConfigArg1)
                                        ],
                                        {skipped, SkipReason}
                                    };
                                {fail, FailReason} ->
                                    {
                                        [{tc_status, {failed, FailReason}} | lists:keydelete(tc_status, 1, ConfigArg1)],
                                        {failed, FailReason}
                                    };
                                OkResult ->
                                    ConfigArg2 =
                                        case init_or_end(Part) of
                                            init when is_list(OkResult) ->
                                                OkResult;
                                            _ ->
                                                ConfigArg1
                                        end,
                                    case proplists:is_defined(tc_status, ConfigArg2) of
                                        true -> {ConfigArg2, OkResult};
                                        false -> {[{tc_status, ok} | ConfigArg2], OkResult}
                                    end
                            catch
                                Class:Reason:Stacktrace ->
                                    Error = format_ct_error(Class, Reason, Stacktrace),
                                    {[{tc_status, {failed, Error}} | ConfigArg1], Error}
                            end,
                        Args = PathArg ++ [PostConfig, Return],
                        call_if_exists_with_fallback_store_state(Hook, post(Part), Args, Return)
                end
            end
        end,
        normalize_part(Part, Fun),
        Hooks
    ),
    %% after the post_per functions we need to handle now failures, and call either on_tc_fail or on_tc_skip
    fun(PathArg, ConfigArg) ->
        [Suite | _] = PathArg,
        Result =
            try WrappedWithPreAndPost(PathArg, ConfigArg) of
                Skip = {skipped, _Reason} ->
                    Skip;
                Fail = {failed, _Reason} ->
                    Fail;
                %% if we don't have a hook setup, we still need to do the conversion from skip/fail to skipped/failed
                {skip, SkipReason} ->
                    {skipped, SkipReason};
                {fail, FailReason} ->
                    {failed, FailReason};
                MaybeConfig ->
                    case init_or_end(Part) of
                        'end' ->
                            %% ends may return any kind of value
                            {ok, ConfigArg};
                        init ->
                            case proplists:get_value(tc_status, MaybeConfig, ok) of
                                ok ->
                                    {ok, lists:keydelete(tc_status, 1, MaybeConfig)};
                                FailOrSkip ->
                                    FailOrSkip
                            end
                    end
            catch
                Class:Reason:Stacktrace -> {failed, {'EXIT', {{Class, Reason}, Stacktrace}}}
            end,
        handle_post_result(Hooks, build_test_name(Part, PathArg), Suite, Result)
    end.

handle_post_result(Hooks, TestName, Suite, Result) ->
    ReverseHooks = lists:reverse(Hooks),
    case Result of
        SkipResult = {skipped, _} ->
            [
                call_if_exists_with_fallback_store_state(
                    Hook, on_tc_skip, [Suite, TestName, SkipResult], ok
                )
             || Hook <- ReverseHooks
            ],
            SkipResult;
        FailResult = {failed, _} ->
            [
                call_if_exists_with_fallback_store_state(
                    Hook, on_tc_fail, [Suite, TestName, FailResult], ok
                )
             || Hook <- ReverseHooks
            ],
            FailResult;
        {ok, Config} ->
            case lists:keyfind(tc_status, 1, Config) of
                false ->
                    Config;
                {tc_status, SkipResult = {skipped, _}} ->
                    [
                        call_if_exists_with_fallback_store_state(
                            Hook, on_tc_skip, [Suite, TestName, SkipResult], ok
                        )
                     || Hook <- ReverseHooks
                    ],
                    SkipResult;
                {tc_status, FailResult = {failed, _}} ->
                    [
                        call_if_exists_with_fallback_store_state(
                            Hook, on_tc_fail, [Suite, TestName, FailResult], ok
                        )
                     || Hook <- ReverseHooks
                    ],
                    FailResult
            end
    end.

-spec format_ct_error(throw | error | exit, Reason, Stacktrace) ->
    {fail, {thrown, Reason, Stacktrace}}
    | {fail, {Reason, Stacktrace}}
    | {fail, Reason}
when
    Reason :: term(), Stacktrace :: erlang:stacktrace().
format_ct_error(throw, Reason, Stacktrace) ->
    {fail, {thrown, Reason, Stacktrace}};
format_ct_error(error, Reason, Stacktrace) ->
    {fail, {Reason, Stacktrace}};
format_ct_error(exit, Reason, Stacktrace) when is_list(Stacktrace) ->
    {fail, {exit, Reason, Stacktrace}}.

-spec build_test_name(part(), [atom()]) -> test_name().
build_test_name(init_per_suite, _Path) ->
    init_per_suite;
build_test_name(end_per_suite, _Path) ->
    end_per_suite;
build_test_name(init_per_group, [_, Group]) ->
    {init_per_group, Group};
build_test_name(end_per_group, [_, Group]) ->
    {end_per_group, Group};
build_test_name(init_per_testcase, [_, Test]) ->
    Test;
build_test_name(init_per_testcase, Path) ->
    [Test, Group | _] = lists:reverse(Path),
    {Group, Test};
build_test_name(end_per_testcase, [_, Test]) ->
    Test;
build_test_name(end_per_testcase, Path) ->
    [Test, Group | _] = lists:reverse(Path),
    {Group, Test}.

get_hook_module({Mod, _}) -> Mod;
get_hook_module(Mod) -> Mod.

get_hook_opts({_, Opts}) -> Opts;
get_hook_opts(_) -> [].

normalize_part(Part, Fun) ->
    SafeFun = get_safe_part(Part, Fun),
    case level(Part) of
        suite -> fun([_Suite], Config) -> SafeFun(Config) end;
        group -> fun([_Suite, Group], Config) -> SafeFun(Group, Config) end;
        testcase -> fun(Path, Config) -> SafeFun(lists:last(Path), Config) end
    end.

%% wrappers because most calls are optional
call_if_exists(Mod, Fun, Args, Default) ->
    case erlang:function_exported(Mod, Fun, erlang:length(Args)) of
        true ->
            erlang:apply(Mod, Fun, Args);
        false ->
            case Default of
                {'$lazy', LazyFun} -> LazyFun();
                _ -> Default
            end
    end.

call_if_exists_with_fallback(Mod, Fun, Args, ReturnDefault) ->
    [_ | FallbackArgs] = Args,
    call_if_exists(Mod, Fun, Args, {'$lazy', fun() -> call_if_exists(Mod, Fun, FallbackArgs, ReturnDefault) end}).

call_if_exists_with_fallback_store_state({Mod, Id}, Fun, Args, ReturnDefault) ->
    {ok, State} = get_state(Id),
    Default =
        case Fun of
            _ when Fun =:= on_tc_fail orelse Fun =:= on_tc_skip -> State;
            _ -> {ReturnDefault, State}
        end,
    CallReturn = call_if_exists_with_fallback(Mod, Fun, Args ++ [State], Default),
    {NewReturn, NewState} =
        case Fun of
            _ when Fun =:= on_tc_fail orelse Fun =:= on_tc_skip -> {ok, CallReturn};
            _ -> CallReturn
        end,
    ok = set_state(Id, NewState),
    NewReturn.

-spec wrapped_id(module(), opts()) -> term().
wrapped_id(Mod, Opts) ->
    case code:ensure_loaded(Mod) of
        {module, Mod} -> ok;
        Error -> error({load_hooks_module, Mod, Error})
    end,
    call_if_exists(Mod, id, [Opts], make_ref()).

-spec wrapped_init(hook(), opts()) -> {non_neg_integer(), hook_state()}.
wrapped_init({Mod, Id}, Opts) ->
    case Mod:init(Id, Opts) of
        {ok, State} -> {0, State};
        {ok, State, Priority} -> {Priority, State};
        Error -> error({hooks_init_error, Error})
    end.

pre(init_per_suite) -> pre_init_per_suite;
pre(init_per_group) -> pre_init_per_group;
pre(init_per_testcase) -> pre_init_per_testcase;
pre(end_per_suite) -> pre_end_per_suite;
pre(end_per_group) -> pre_end_per_group;
pre(end_per_testcase) -> pre_end_per_testcase.

post(init_per_suite) -> post_init_per_suite;
post(init_per_group) -> post_init_per_group;
post(init_per_testcase) -> post_init_per_testcase;
post(end_per_suite) -> post_end_per_suite;
post(end_per_group) -> post_end_per_group;
post(end_per_testcase) -> post_end_per_testcase.

level(init_per_suite) -> suite;
level(init_per_group) -> group;
level(init_per_testcase) -> testcase;
level(end_per_suite) -> suite;
level(end_per_group) -> group;
level(end_per_testcase) -> testcase.

init_or_end(init_per_suite) -> init;
init_or_end(init_per_group) -> init;
init_or_end(init_per_testcase) -> init;
init_or_end(end_per_suite) -> 'end';
init_or_end(end_per_group) -> 'end';
init_or_end(end_per_testcase) -> 'end'.

get_safe_part(Part, Fun) ->
    case is_exported(Fun) of
        true -> Fun;
        false -> dummy(Part)
    end.

dummy(init_per_suite) -> fun(Config) -> Config end;
dummy(init_per_group) -> fun(_, Config) -> Config end;
dummy(init_per_testcase) -> fun(_, Config) -> Config end;
dummy(end_per_suite) -> fun(_) -> ok end;
dummy(end_per_group) -> fun(_, _) -> ok end;
dummy(end_per_testcase) -> fun(_, _) -> ok end.

is_exported(Fun) ->
    case maps:from_list(erlang:fun_info(Fun)) of
        #{
            type := external,
            module := Module,
            name := Function,
            arity := Arity
        } ->
            erlang:function_exported(Module, Function, Arity);
        _ ->
            false
    end.
