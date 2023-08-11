%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Setup functions for logger and CT printing facilities
%%% @end
%%% % @format

-module(ct_daemon_logger).

-include_lib("kernel/include/logger.hrl").

%% Public API
-export([setup/2]).

%% @doc mocks for ct_logs functions
-spec setup(file:filename_all(), boolean()) -> ok.
setup(OutputDir, InstrumentCTLogs) ->
    LogFile = test_logger:get_log_file(OutputDir, ct_daemon),
    ok = test_logger:configure_logger(LogFile),

    %% check is we need to instrument ct_logs
    %% this somehow crashes the node startup if CT runs on the
    %% controlling node
    case InstrumentCTLogs of
        true ->
            meck:new(ct_logs, [passthrough, no_link]),
            meck:expect(ct_logs, tc_log, fun tc_log/3),
            meck:expect(ct_logs, tc_log, fun tc_log/4),
            meck:expect(ct_logs, tc_log, fun tc_log/5),
            meck:expect(ct_logs, tc_print, fun tc_print/3),
            meck:expect(ct_logs, tc_print, fun tc_print/4),
            meck:expect(ct_logs, tc_print, fun tc_print/5),
            meck:expect(ct_logs, tc_pal, fun tc_pal/3),
            meck:expect(ct_logs, tc_pal, fun tc_pal/4),
            meck:expect(ct_logs, tc_pal, fun tc_pal/5);
        _ ->
            ok
    end,
    ok.

tc_log(Category, Format, Args) ->
    tc_print(Category, 1000, Format, Args).

tc_log(Category, Importance, Format, Args) ->
    tc_print(Category, Importance, Format, Args, []).

tc_log(Category, Importance, Format, Args, _Opts) ->
    LogMessage = lists:flatten(
        io_lib:format("[ct_logs][~p][~p] ~s", [Category, Importance, Format])
    ),
    ?LOG_INFO(LogMessage, Args).

tc_print(Category, Format, Args) ->
    tc_print(Category, 1000, Format, Args).

tc_print(Category, Importance, Format, Args) ->
    tc_print(Category, Importance, Format, Args, []).

tc_print(_Category, _Importance, Format, Args, _Opts) ->
    FormatWithTimesStamp = io_lib:format("[~s] ~s\n", [timestamp(), Format]),
    FinalFormat = lists:flatten(FormatWithTimesStamp),
    io:format(FinalFormat, Args).

tc_pal(Category, Format, Args) ->
    tc_print(Category, 1000, Format, Args).

tc_pal(Category, Importance, Format, Args) ->
    tc_print(Category, Importance, Format, Args, []).

tc_pal(Category, Importance, Format, Args, Opts) ->
    ct_logs:tc_log(Category, Importance, Format, Args, [no_css | Opts]),
    tc_print(Category, Importance, Format, Args, Opts).

timestamp() ->
    calendar:system_time_to_rfc3339(erlang:system_time(second)).
