%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Documentation for ct_daemon_node, ways to use
%%%   it, ways to break it, etc. etc
%%% @end
%%% % @format

-module(ct_daemon_node).

-include_lib("kernel/include/logger.hrl").

%% Public API
-export([start/0, start/1, stop/0, alive/0, get_node/0]).

-export([node_main/1, get_domain_type/0]).

-define(LOG_BASE, "/tmp/ct_daemon").

-type config() :: #{
    type := shortnames | longnames,
    name := node(),
    cookie := atom(),
    options := [opt()]
}.

-type opt() ::
    {multiply_timetraps, number() | infinity}
    | {ct_hooks, [atom() | {atom(), [term()]}]}.

-export_type([config/0]).

%% @doc start node for running tests in isolated way and keep state
-spec start() -> ok.
start() ->
    NodeName = list_to_atom(
        lists:flatten(io_lib:format("test~s-atn@localhost", [random_name()]))
    ),
    StartConfig = #{
        type => shortnames,
        name => NodeName,
        cookie => ct_runner:cookie(),
        options => []
    },
    start(StartConfig).

%% @doc start node for running tests in isolated way and keep state
-spec start(config()) -> ok | {error, {crash_on_startup, integer()}}.
start(
    _Config = #{
        type := Type,
        name := Node,
        cookie := Cookie,
        options := Options
    }
) ->
    RandomName = random_name(),
    ok = ensure_distribution(Type, RandomName, Cookie),
    %% get code paths from current node
    CodePaths = code:get_path(),
    ConfigFiles = get_config_files(),
    OutputDir = gen_output_dir(RandomName),
    FullOptions = [{output_dir, OutputDir} | Options],
    Args = build_daemon_args(Type, Node, Cookie, FullOptions, OutputDir),
    % Replay = maps:get(replay, Config, false),
    % We should forward emu flags here,
    % see T129435667
    Port = ct_runner:start_test_node(
        os:find_executable("erl"),
        CodePaths,
        ConfigFiles,
        OutputDir,
        [{args, Args}, {cd, OutputDir}],
        false
    ),
    %% wait for the ct_daemon gen_server to be started
    true = erlang:register(?MODULE, self()),
    port_loop(Port, []).

port_loop(Port, Acc) ->
    receive
        {Port, {data, {eol, Line}}} ->
            port_loop(Port, [Line | Acc]);
        ready ->
            true = erlang:unregister(?MODULE),
            ok = global:sync();
        {Port, {exit_status, N}} ->
            ?LOG_DEBUG("Test Node Crashed on Startup: ~n~s~n", [lists:join("\n", lists:reverse(Acc))]),
            {error, {crash_on_startup, N}}
    end.

-spec stop() -> node().
stop() ->
    %% the gen_server might be blocked, we spawn a process on the
    %% remote note that exectues `erlang:halt()' to bypass

    Node = get_node(),

    %% monitore node
    true = erlang:monitor_node(Node, true),
    %% kill node
    _Pid = erlang:spawn(Node, fun() -> erlang:halt() end),
    %% wait for node to come down
    receive
        {nodedown, Node} -> ok
    end,
    Node.

-spec get_node() -> node().
get_node() ->
    case alive() of
        true -> erlang:node(get_runner_pid());
        false -> error(not_running)
    end.

-spec alive() -> boolean().
alive() ->
    erlang:is_pid(get_runner_pid()).

%% @doc node main entry point
-spec node_main([node()]) -> no_return().
node_main([Parent, OutputDirAtom, InstrumentCTLogs]) ->
    ok = application:load(test_exec),
    OutputDir = erlang:atom_to_list(OutputDirAtom),

    %% set stack trace to 20
    erlang:system_flag(backtrace_depth, 20),

    %% setup logger and prepare IO
    ok = ct_daemon_logger:setup(OutputDir, InstrumentCTLogs),

    true = net_kernel:connect_node(Parent),

    {ok, {RunnerPid, RunnerMonRef}} = ct_daemon_runner:start_monitor(Parent, OutputDir),
    {ok, {HooksPid, HooksMonRef}} = ct_daemon_hooks:start_monitor(),

    true = erlang:monitor_node(Parent, true),
    {?MODULE, Parent} ! ready,

    %% block unless parent node dies or ct_daemon_runner
    receive
        {nodedown, _} ->
            ?LOG_INFO("parent node went down, terminating test node", []),
            ok;
        {'DOWN', RunnerMonRef, process, RunnerPid, _} ->
            ?LOG_INFO("ct_daemon_runner went down, terminating test node", []),
            ok;
        {'DOWN', HooksMonRef, process, HooksPid, _} ->
            ?LOG_INFO("ct_daemon_hooks went down, terminating test node", []),
            ok
    end,
    test_logger:flush(),
    erlang:halt(0).

%% internal
-spec ensure_distribution(longnames | shortnames, RandomName :: string(), Cookie :: atom()) -> ok.
ensure_distribution(Type, RandomName, Cookie) ->
    case erlang:node() of
        'nonode@nohost' ->
            % distribution is not started, ensure epmd is
            (erl_epmd:names("localhost") =:= {error, address}) andalso
                ([] = os:cmd("epmd -daemon")),
            Name = list_to_atom(
                lists:flatten(
                    io_lib:format("ct_daemon~s", [RandomName])
                )
            ),
            {ok, _Pid} = net_kernel:start(Name, #{name_domain => Type}),
            ok;
        _ ->
            %% check that the domain is correct
            Type = get_domain_type(),
            ok
    end,
    true = erlang:set_cookie(Cookie),
    ok.

-spec build_daemon_args(shortnames | longnames, node(), atom(), [opt()], file:filename_all()) ->
    [string()].
build_daemon_args(Type, Node, Cookie, Options, OutputDir) ->
    DistArg =
        case Type of
            longnames -> "-name";
            shortnames -> "-sname"
        end,
    InstrumentCTLogs = erlang:whereis(ct_logs) =:= undefined,
    [
        DistArg,
        convert_atom_arg(Node),
        "-setcookie",
        convert_atom_arg(Cookie),
        "-test_exec",
        "daemon_options",
        lists:flatten(io_lib:format("~w", [Options])),
        "-s",
        convert_atom_arg(?MODULE),
        "node_main",
        convert_atom_arg(erlang:node()),
        OutputDir,
        convert_atom_arg(InstrumentCTLogs)
    ].

-spec convert_atom_arg(atom()) -> string().
convert_atom_arg(Arg) ->
    lists:flatten(io_lib:format("~s", [Arg])).

-spec get_config_files() -> [file:filename_all()].
get_config_files() ->
    _ = application:load(test_exec),
    PrivDir = code:priv_dir(test_exec),
    [
        ConfigFile
     || ConfigFile <- filelib:wildcard(filename:join(PrivDir, "*")),
        filename:extension(ConfigFile) =:= ".config"
    ].

-spec gen_output_dir(RandomName :: string()) -> file:filename().
gen_output_dir(RandomName) ->
    BaseDir =
        case application:get_env(test_exec, ct_daemon_log_dir, undefined) of
            undefined ->
                ?LOG_BASE;
            LogDir ->
                LogDir
        end,
    filename:join([
        BaseDir,
        "tests",
        RandomName
    ]).

-spec random_name() -> io_lib:chars().
random_name() ->
    io_lib:format("~b-~b~s", [rand:uniform(100000), erlang:unique_integer([positive, monotonic]), os:getpid()]).

-spec get_domain_type() -> longnames | shortnames.
get_domain_type() ->
    %% now the docs say this returns shortnames or longnames for field
    %% name_domain, but the code says domain_type long or short
    %% Upstream code agrees with the documentation, and until we have
    %% updated to at least 25 this code supports both versions.
    case net_kernel:get_state() of
        #{domain_type := short} -> shortnames;
        #{name_domain := shortnames} -> shortnames;
        #{domain_type := long} -> longnames;
        #{name_domain := longnames} -> longnames
    end.

-spec get_runner_pid() -> pid() | undefined.
get_runner_pid() ->
    global:whereis_name(ct_daemon_runner:name(node())).
