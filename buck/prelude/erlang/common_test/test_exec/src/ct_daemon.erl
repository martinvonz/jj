%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Daemon for running Common Test in an iterative way from an Erlang Shell
%%% @end
%%% % @format

-module(ct_daemon).

-export([
    start/0, start/1,
    stop/0,
    alive/0,
    run/1,
    list/0, list/1,
    ping/0,
    push_module/1,
    push_paths/1,
    set_gl/0,
    discover/1,
    load_changed/0,
    setup_state/0,
    output_dir/0
]).

%% @doc start a test-node with random name and shortname
-spec start() -> ok.
start() ->
    ct_daemon_node:start().

%% @doc starts the test node with the given distribution mode and node name
-spec start(ct_daemon_node:config()) -> ok.
start(NodeInfo) ->
    ct_daemon_node:start(NodeInfo).

%% @doc stops the test node
-spec stop() -> ok.
stop() ->
    ct_daemon_node:stop().

%% @doc returns if the test-node is alive
-spec alive() -> boolean().
alive() ->
    ct_daemon_node:alive().

%% @doc run test from scratch
-spec run(
    Test ::
        string()
        | non_neg_integer()
        | {discovered, [#{suite => module(), name => string()}]}
) ->
    #{string() => ct_daemon_runner:run_result()} | ct_daemon_runner:discover_error().
run(Test) ->
    do_call({run, Test}).

-spec ping() -> {pong, term()}.
ping() ->
    do_call(ping).

-spec load_changed() -> [module()].
load_changed() ->
    do_call(load_changed).

set_gl() ->
    do_call({gl, group_leader()}).

-spec list() -> [{module(), [{non_neg_integer(), string()}]}].
list() ->
    do_call(list).

-spec list(RegEx :: string()) ->
    [{module(), [{non_neg_integer(), string()}]}] | {invalid_regex, {string, non_neg_integer()}}.
list(RegEx) ->
    case re:compile(RegEx) of
        {ok, Pattern} ->
            [
                {Suite, [Test || Test = {_Id, Name} <- Tests, re:run(Name, Pattern) =/= nomatch]}
             || {Suite, Tests} <- list()
            ];
        {error, ErrSpec} ->
            {invalid_regex, ErrSpec}
    end.

-spec discover(pos_integer() | string()) ->
    #{suite := module(), name := string()}
    | ct_daemon_runner:discover_error().
discover(RegExOrId) ->
    do_call({discover, RegExOrId}).

-spec setup_state() -> [atom()] | undefined.
setup_state() ->
    case alive() of
        true ->
            do_call(setup);
        _ ->
            undefined
    end.

-spec output_dir() -> file:filename_all() | undefined.
output_dir() ->
    do_call(output_dir).

-spec push_paths(Paths :: [file:filename_all()]) -> ok.
push_paths(Paths) ->
    case alive() of
        true ->
            do_cast({code_paths, Paths});
        false ->
            ok
    end.

-spec push_module(module()) -> ok.
push_module(Module) ->
    case alive() of
        true ->
            do_cast({load_module, Module});
        false ->
            ok
    end.

%% call abstraction:
do_call(Request) ->
    try
        gen_server:call({global, ct_daemon_runner:name(node())}, Request, infinity)
    catch
        exit:{noproc, {gen_server, call, _}} ->
            node_down
    end.

do_cast(Request) ->
    gen_server:cast({global, ct_daemon_runner:name(node())}, Request).
