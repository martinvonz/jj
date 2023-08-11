%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%@doc
%% Simple trampoline for ct_run.
%% Notably allows us to call post/pre method on the node if needed, e.g for coverage.

-module(ct_executor).

-include_lib("kernel/include/logger.hrl").
-include_lib("common/include/buck_ct_records.hrl").

-export([run/1]).

% Time we give the beam to close off, in ms.
-define(INIT_STOP_TIMEOUT, 5000).

run(Args) when is_list(Args) ->
    ExitCode =
        try
            {CtExecutorArgs, CtRunArgs} = parse_arguments(Args),
            debug_print("~p", [#{ct_exec_args => CtExecutorArgs, ct_run_args => CtRunArgs}]),
            {_, OutputDir} = lists:keyfind(output_dir, 1, CtExecutorArgs),
            ok = test_logger:set_up_logger(OutputDir, ?MODULE),

            %% log arguments into ct_executor.log
            ?LOG_INFO("raw args: ~p", [Args]),
            ?LOG_INFO("executor args: ~p", [CtExecutorArgs]),
            ?LOG_INFO("CtRunArgs: ~p", [CtRunArgs]),

            % Until this point the logger is not set up so we cannot log.
            % Therefore we used io:format to forward information to the
            % process calling it (ct_runner).
            try
                % We consult all the .app files to load the atoms.
                % This solution is less than optimal and should be addressed
                % T120903856
                PotentialDotApp = [
                    filename:join(Dep, filename:basename(filename:dirname(Dep)) ++ ".app")
                 || Dep <- code:get_path()
                ],
                [file:consult(DotApp) || DotApp <- PotentialDotApp, filelib:is_regular(DotApp)],
                {_, Suite} = lists:keyfind(suite, 1, CtExecutorArgs),
                ProviderInitState = #init_provider_state{output_dir = OutputDir, suite = Suite},
                Providers0 =
                    case lists:keyfind(providers, 1, CtExecutorArgs) of
                        false ->
                            [];
                        {_, Providers} ->
                            [
                                buck_ct_provider:do_init(Provider, ProviderInitState)
                             || Provider <- Providers
                            ]
                    end,
                %% get longer stack traces
                erlang:system_flag(backtrace_depth, 20),
                ?LOG_DEBUG("ct_run called with arguments ~p ~n", [CtRunArgs]),
                Providers1 = [buck_ct_provider:do_pre_running(Provider) || Provider <- Providers0],
                {ok, IoBuffer} = io_buffer:start_link(),
                register(cth_tpx_io_buffer, IoBuffer),
                %% set global timeout
                Result = ct:run_test(CtRunArgs),
                ?LOG_DEBUG("ct_run finished with result ~p ~n", [Result]),
                Providers2 = [buck_ct_provider:do_post_running(Provider) || Provider <- Providers1],
                [buck_ct_provider:do_terminate(Provider) || Provider <- Providers2],
                0
            catch
                Class:Reason:Stack ->
                    ?LOG_ERROR("ct executor failed due to ~ts\n", [
                        erl_error:format_exception(Class, Reason, Stack)
                    ]),
                    2
            after
                test_logger:flush()
            end
        catch
            % Catch an exception that happens before logging is set up.
            % Will forward the exception to the process that opened the port (ct_runner).
            Class1:Reason1:Stack1 ->
                io:format("~ts\n", [erl_error:format_exception(Class1, Reason1, Stack1)]),
                1
        end,
    case ExitCode of
        0 ->
            init:stop(0),
            receive
            after ?INIT_STOP_TIMEOUT ->
                ?LOG_ERROR(
                    io_lib:format("~p failed to terminate within ~c millisecond", [
                        ?MODULE, ?INIT_STOP_TIMEOUT
                    ])
                ),
                erlang:halt(0)
            end;
        _ ->
            erlang:halt(ExitCode)
    end.

-spec parse_arguments([string()]) -> {proplists:proplist(), [term()]}.
parse_arguments(Args) ->
    % The logger is not set up yet.
    % This will be sent to the program executing it (ct_runner),
    % that will log it in its own log.
    debug_print("CT executor called with ~p~n", [Args]),
    ParsedArgs = lists:map(
        fun(StrArgs) ->
            buck_ct_parser:parse_str(StrArgs)
        end,
        Args
    ),
    debug_print("Parsed arguments ~p~n", [ParsedArgs]),
    % We split the arguments between those that go to ct_run and those that are for
    % ct_executor
    % the args passed to ct are to be found after the --ct-args
    split_args(ParsedArgs).

% @doc Splits the argument before those that happens
% before ct_args (the executor args) amd those after
% (the args for ct_run).
split_args(Args) -> split_args(Args, [], []).

split_args([ct_args | Args], CtExecutorArgs, []) -> {lists:reverse(CtExecutorArgs), Args};
split_args([Arg | Args], CtExecutorArgs, []) -> split_args(Args, [Arg | CtExecutorArgs], []);
split_args([], CtExecutorArgs, []) -> {lists:reverse(CtExecutorArgs), []}.

debug_print(Fmt, Args) ->
    case os:getenv("ERLANG_BUCK_DEBUG_PRINT") of
        false -> io:format(Fmt, Args);
        "disabled" -> ok;
        _ -> io:format(Fmt, Args)
    end.
