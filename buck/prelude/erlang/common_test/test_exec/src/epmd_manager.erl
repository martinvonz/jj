%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

%% @doc This module interfaces with the epmd daemon. It allows one to start/stop one for
%% each suite execution.

-module(epmd_manager).

-include_lib("common/include/buck_ct_records.hrl").

%% UI methods
-export([start_link/1, get_epmd_out_path/1]).

%% gen_server callbacks
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2, handle_continue/2]).

-behaviour(gen_server).

%% @doc Find a new port and starts this epmd daemon on this new port, ensures it is up and working,
%% and set up the env variable ERL_EPMD_PORT to the port this daemon is working.
-spec start_link(file:filename_all()) -> {ok, reference()} | {error, term()}.
start_link(#test_env{} = TestEnv) ->
    gen_server:start_link(?MODULE, [TestEnv], [debugs, [trace, log]]).

%% ---------------- gen_server callbacks---------------

init([TestEnv]) ->
    process_flag(trap_exit, true),
    {ok, #{}, {continue, {start_epmd, TestEnv}}}.

handle_continue({start_epmd, #test_env{output_dir = OutputDir} = TestEnv}, _State) ->
    EpmdOutPath = get_epmd_out_path(OutputDir),
    case start_epmd(EpmdOutPath) of
        {ok, Port, PortEpmd, LogHandle} ->
            {ok, _Pid} = test_exec_sup:start_ct_runner(TestEnv, Port),
            {noreply, #{epmd_port => PortEpmd, log_handle => LogHandle}};
        Error ->
            {stop, {epmd_start_failed, Error}, #{}}
    end.

handle_cast(_Request, State) -> {ok, State}.

handle_call(_Request, _From, State) -> {reply, ok, State}.

handle_info({PortEpmd, {exit_status, ExitStatus}}, #{epmd_port := PortEpmd} = State) ->
    {stop, {epmd_crashed, ExitStatus}, State};
handle_info({PortEpmd, closed}, #{epmd_port := PortEpmd} = State) ->
    {stop, epmd_port_closed, State};
handle_info({'EXIT', PortEpmd, Reason}, #{epmd_port := PortEpmd} = State) ->
    {stop, {epmd_exit, Reason}, State};
handle_info({PortEpmd, {data, Data}}, #{epmd_port := PortEpmd, log_handle := LogHandle} = State) ->
    log_input_data(Data, LogHandle),
    {noreply, State};
handle_info(_Info, State) ->
    {noreply, State}.

terminate(_Reason, #{epmd_port := EpmdPort}) ->
    test_exec:kill_process(EpmdPort);
terminate(_Reason, _State) ->
    ok.

%% --------------  Utilities methods ------------------

%% @doc Find a new port and starts this epmd daemon on this new port, then ensures it is up and working.
%%  We may have to retry in case the port becomes busy between the time
%% it is discovered and the time the epmd is ran.
-spec start_epmd(file:filename()) ->
    {ok, inet:port_number(), erlang:port(), pid()} | {error, {epmd_starting_failed, term()}}.
start_epmd(EpmdOutPath) -> start_epmd(EpmdOutPath, 3, no_error).
-spec start_epmd(file:filename(), integer(), term()) ->
    {ok, inet:port_number(), erlang:port(), pid()} | {error, {epmd_starting_failed, term()}}.
start_epmd(EpmdOutPath, Attempts, _Error) when Attempts > 0 ->
    case find_free_port() of
        {ok, Port} ->
            case start_epmd_instance(Port, EpmdOutPath) of
                {ok, PortEpmd, LogHandle} -> {ok, Port, PortEpmd, LogHandle};
                {failed, Reason} -> start_epmd(EpmdOutPath, Attempts - 1, Reason)
            end;
        Error ->
            start_epmd(EpmdOutPath, Attempts - 1, Error)
    end;
start_epmd(_LogDir, 0, Error) ->
    {error, {epmd_starting_failed, Error}}.

%% @doc Finds a free TCP port.
%% This methods relies on a special behavior of gen_tcp:listen/0,
%% See https://www.erlang.org/doc/man/gen_tcp.html#listen-2
-spec find_free_port() -> inet:port_number().
find_free_port() ->
    case gen_tcp:listen(0, [inet]) of
        {ok, ListenSocket} ->
            Port = inet:port(ListenSocket),
            gen_tcp:close(ListenSocket),
            Port;
        {error, Reason} ->
            {error, Reason}
    end.

%% @doc Starts the epmd daemon on the given port,
%% and writes stdout, stderr to files in the LogDir.
-spec start_epmd_instance(inet:port(), file:filename_all()) -> {failed, term()} | {ok, port()}.
start_epmd_instance(Port, EpmdOutPath) ->
    %% Note on the -d flag from `man 1 epmd`:
    %% Enables debug output. The more -d flags specified, the more
    %% debug output you will get (to a certain limit). This option is
    %% most useful when the epmd daemon is not started as a daemon.
    LogHandle = get_log_handle(EpmdOutPath),
    ProcessPort = erlang:open_port(
        {
            spawn,
            string:join(
                [
                    "epmd",
                    "-d",
                    "-d",
                    "-port",
                    integer_to_list(Port)
                ],
                " "
            )
        },
        [stderr_to_stdout, exit_status, use_stdio]
    ),
    case listen_loop(ProcessPort, LogHandle) of
        ok ->
            {ok, ProcessPort, LogHandle};
        Error ->
            exit(LogHandle, closing_epmd),
            Error
    end.

-spec listen_loop(port(), pid()) -> {failed, term()} | ok.
listen_loop(ProcessPort, LogHandle) ->
    receive
        {ProcessPort, {exit_status, Exit}} ->
            {failed, {epmd_exit, Exit}};
        {ProcessPort, {data, Data}} ->
            log_input_data(Data, LogHandle),
            case string:find(Data, "entering the main select() loop") of
                nomatch -> listen_loop(ProcessPort, LogHandle);
                _ -> ok
            end
    after 1000 ->
        test_exec:kill_process(ProcessPort),
        {failed, timeout}
    end.

-spec get_log_handle(file:name_all()) -> pid().
get_log_handle(EpmdOutPath) ->
    case filelib:is_file(EpmdOutPath) of
        true -> ok = file:delete(EpmdOutPath);
        false -> ok
    end,
    {ok, LogHandle} = file:open(EpmdOutPath, [write]),
    LogHandle.

-spec log_input_data(binary(), pid()) -> ok.
log_input_data(Data, LogHandle) ->
    io:format(LogHandle, "~ts", [Data]).

get_epmd_out_path(OutputDir) ->
    filename:join(OutputDir, "epmd_out.log").
