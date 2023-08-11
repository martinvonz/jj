-module(cth_tpx_server).
-behaviour(gen_server).

%% Public API
-export([
    start_link/1,
    get/1,
    modify/2
]).

%% gen_server callbacks
-export([
    init/1,
    handle_call/3,
    handle_cast/2,
    handle_info/2
]).

-export_type([
    handle/0
]).

-type handle() :: pid().


%% ---- PUBLIC API ---------
-spec start_link(InitialState :: term()) -> handle().
start_link(InitialState) ->
    {ok, Handle} = gen_server:start_link(?MODULE, InitialState, []),
    Handle.

-spec get(Handle :: handle()) -> CurrentState :: term().
get(Handle) ->
    gen_server:call(Handle, get).

-spec modify(Handle :: handle(), Fun :: fun((State) -> {A, State})) -> A.
modify(Handle, Fun) ->
    gen_server:call(Handle, {modify, Fun}).


%% ---- gen_server callbacks ----------

-spec init(InitialState :: State) -> {ok, State}.
init(InitialState) ->
    {ok, InitialState}.

handle_call(get, _From, State) ->
    {reply, State, State};
handle_call({modify, Fun}, _From, State) ->
    {A, NewState} = Fun(State),
    {reply, A, NewState}.

handle_cast(_, State) ->
    {noreply, State}.

handle_info(_, State) ->
    {noreply, State}.
