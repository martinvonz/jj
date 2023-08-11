%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%% % @format
-module(test_logger).
-compile(warn_missing_spec).

-export([set_up_logger/2, set_up_logger/3, flush/0, get_std_out/2, get_log_file/2, configure_logger/1]).

-spec set_up_logger(file:filename(), atom()) -> ok.
set_up_logger(LogDir, AppName) ->
    set_up_logger(LogDir, AppName, false).

-spec set_up_logger(file:filename(), atom(), boolean()) -> ok.
set_up_logger(LogDir, AppName, StandaloneConfig) ->
    Log = get_log_file(LogDir, AppName),
    filelib:ensure_dir(Log),
    StdOut = get_std_out(LogDir, AppName),
    filelib:ensure_dir(StdOut),
    {ok, LogFileOpened} = file:open(StdOut, [write]),
    case StandaloneConfig of
        true ->
            [logger:remove_handler(Id) || Id <- logger:get_handler_ids()];
        false ->
            group_leader(
                LogFileOpened, self()
            )
    end,
    configure_logger(Log).

-spec configure_logger(file:filename()) -> ok.
configure_logger(LogFile) ->
    ok = logger:set_primary_config(#{
        level => all,
        filter_default => log
    }),
    ok = logger:add_handler(
        file_handler, logger_std_h, #{
            config => #{
                file => LogFile,
                filesync_repeat_interval => 100
            },
            filter_default => log,
            formatter =>
                {logger_formatter, #{
                    template => [
                        time,
                        " ",
                        pid,
                        {file, [" ", file], []},
                        {line, [":", line], []},
                        " == ",
                        level,
                        ": ",
                        msg,
                        "\n"
                    ]
                }}
        }
    ).

-spec flush() -> ok | {error, term()}.
flush() ->
    logger_std_h:filesync(file_handler).

-spec get_std_out(file:filename(), atom()) -> file:filename().
get_std_out(LogDir, AppName) ->
    filename:join(LogDir, io_lib:format("~p.stdout.txt", [AppName])).

-spec get_log_file(file:filename(), atom()) -> file:filename().
get_log_file(LogDir, AppName) ->
    filename:join(LogDir, io_lib:format("~p.log", [AppName])).
