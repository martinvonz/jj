%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% gen_server holding state between test runs
%%% @end
%%% % @format

-module(ct_daemon_runner).

-include_lib("kernel/include/logger.hrl").

-behavior(gen_server).

%% gen_server API
-export([init/1, handle_call/3, handle_cast/2, handle_info/2]).

%% Public API
-export([start_monitor/2, name/1]).

-type state() :: #{
    enumerated_tests => #{non_neg_integer() => string()},
    output_dir => file:filename_all(),
    setup => ct_daemon_core:setup()
}.

-type discover_error() ::
    {error,
        {ambiguous_test_regex, [{module(), [{non_neg_integer(), string()}]}]}
        | {id_not_found, non_neg_integer()}
        | not_listed_yet
        | invalid_regex
        | {invalid_regex, {string(), non_neg_integer()}}}.

-export_type([discover_error/0]).

start_monitor(Node, OutputDir) ->
    gen_server:start_monitor(
        {global, name(Node)},
        ?MODULE,
        [#{output_dir => OutputDir, setup => undefined}],
        []
    ).

%% @doc global name based on calling node
-spec name(node()) -> atom().
name(Node) ->
    erlang:list_to_atom(lists:flatten(io_lib:format("~s-~s", [Node, ?MODULE]))).

%% gen_server for keeping state
-spec init([state()]) -> {ok, state()}.
init([InitState]) ->
    {ok, InitState}.

handle_call(ping, _From, State) ->
    {reply, {pong, State}, State};
handle_call(list, _From, State) ->
    Tests = ct_daemon_core:list(),
    list_result(Tests, State);
handle_call({run, RegExOrTestIdOrDiscovered}, _From, State) ->
    try
        case get_tests(RegExOrTestIdOrDiscovered, State) of
            E = {error, _} ->
                {reply, E, State};
            Tests ->
                {CollectedResults, EndState} = lists:foldl(
                    fun(Test, {Results, InState}) ->
                        {TestResult, OutState} = run_test(Test, InState),
                        {Results#{Test => TestResult}, OutState}
                    end,
                    {#{}, State},
                    Tests
                ),
                {reply, CollectedResults, EndState}
        end
    catch
        Err:R:ST ->
            {reply, {error, {Err, R, ST}}, State}
    end;
handle_call({discover, RegExOrTestId}, _From, State) ->
    case discover_test(RegExOrTestId, State) of
        E = {error, _} ->
            {reply, E, State};
        Tests ->
            {reply,
                [
                    ct_daemon_core:from_qualified(Test)
                 || Test <- Tests
                ],
                State}
    end;
handle_call({gl, GL}, _From, State) ->
    {reply, erlang:group_leader(GL, self()), State};
handle_call(load_changed, _From, State) ->
    {reply, load_changed_modules(), State};
handle_call(setup, _From, #{setup := #{setup_state := {Names, _}}} = State) ->
    {reply, Names, State};
handle_call(setup, _From, State) ->
    {reply, undefined, State};
handle_call(output_dir, _From, State) ->
    DaemonOptions = application:get_env(test_exec, daemon_options, []),
    {reply, proplists:get_value(output_dir, DaemonOptions), State};
handle_call(Request, _From, State) ->
    {reply, Request, State}.

handle_cast({code_paths, Paths}, State) ->
    ?LOG_DEBUG("addign code paths ~p", [Paths]),
    ok = code:add_paths(Paths),
    {noreply, State};
handle_cast({load_module, Module}, State) ->
    reload_module(Module),
    {noreply, State};
handle_cast(Request, State) ->
    ?LOG_INFO("unrecognized cast: ~p state: ~p", [Request, State]),
    erlang:error(not_implemented).

handle_info(_Info, State) ->
    {noreply, State}.

%% internal
-spec get_tests(
    non_neg_integer()
    | string()
    | {discovered, [#{suite => module(), name => string()}]},
    state()
) ->
    discover_error() | [string()].
get_tests({discovered, Discovered}, _State) ->
    [ct_daemon_core:to_qualified(Test) || Test <- Discovered];
get_tests(RegExOrTestId, State) ->
    discover_test(RegExOrTestId, State).

-spec discover_test(non_neg_integer() | string(), state()) -> discover_error() | [string()].
discover_test(TestId, State) when erlang:is_integer(TestId) ->
    case State of
        #{enumerated_tests := #{TestId := Test}} ->
            [Test];
        #{enumerate_tests := _} ->
            {error, {id_not_found, TestId}};
        _ ->
            {error, not_listed_yet}
    end;
discover_test(RegEx, _State) when erlang:is_list(RegEx) ->
    Listing = maps:values(ct_daemon_core:list()),
    case re:compile(RegEx) of
        {ok, Pattern} ->
            [Test || Test <- lists:concat(Listing), re:run(Test, Pattern) =/= nomatch];
        {error, ErrSpec} ->
            {error, {invalid_regex, ErrSpec}}
    end;
discover_test(_, _) ->
    {error, invalid_regex}.

-spec list_result(#{module() => [string()]}, state()) ->
    {reply, [{module(), [{non_neg_integer(), string()}]}], state()}.
list_result(Tests, State) ->
    {_, EnumeratedTests} = enumerate_tests(Tests),
    NewState = State#{enumerated_tests => flatten_enumerated_tests(EnumeratedTests)},
    {reply, EnumeratedTests, NewState}.

-spec enumerate_tests(#{module() => [string()]}) -> [{module(), [{non_neg_integer(), string()}]}].
enumerate_tests(Tests) ->
    maps:fold(
        fun(Suite, SuiteTests, {InCounter, Acc}) ->
            {InCounter + erlang:length(SuiteTests), [
                {Suite, lists:enumerate(InCounter, SuiteTests)} | Acc
            ]}
        end,
        {0, []},
        Tests
    ).

-spec flatten_enumerated_tests([{module(), [{non_neg_integer(), string()}]}]) ->
    #{non_neg_integer() => string()}.
flatten_enumerated_tests(Tests) ->
    maps:from_list(lists:flatten([SuiteTests || {_, SuiteTests} <- Tests])).

-spec run_test(string(), state()) -> {ct_daemon_core:run_result(), state()}.
run_test(Test, State = #{output_dir := OutputDir, setup := InSetupState}) ->
    #{suite := Suite, name := Name} = ct_daemon_core:from_qualified(Test),
    Spec = test_runner:parse_test_name(Name, Suite),

    ?LOG_INFO("discovered test ~p with spec ~p", [Name, Spec]),

    {Result, OutSetupState} = ct_daemon_core:run_test(Spec, InSetupState, OutputDir),

    {Result, State#{setup => OutSetupState}}.

-spec load_changed_modules() -> ok.
load_changed_modules() ->
    ChangedModules = lists:usort([
        M
     || {M, _} <- code:all_loaded(), module_modified(M) orelse module_interpreted(M)
    ]),
    [reload_module(Module) || Module <- ChangedModules],
    %% reinterprete debugged modules
    [int:i(Module) || Module <- int:interpreted()],
    ChangedModules.

module_modified(Mod) ->
    case code:is_loaded(Mod) of
        {file, preloaded} ->
            false;
        {file, BeamPath} ->
            case code:is_sticky(Mod) of
                false -> module_modified(Mod, BeamPath);
                true -> false
            end;
        _ ->
            false
    end.

module_modified(Mod, BeamPath) ->
    LoadedMD5 = Mod:module_info(md5),
    case beam_lib:md5(BeamPath) of
        {ok, {Mod, DiskMD5}} ->
            LoadedMD5 =/= DiskMD5;
        _ ->
            false
    end.

module_interpreted(Mod) ->
    lists:member(Mod, int:interpreted()).

reload_module(Module) ->
    code:purge(Module),
    code:load_file(Module).
