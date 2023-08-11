%% #!/usr/bin/env escript
%% -*- erlang -*-
%%! +sbtu

%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc
%%%  Copy ERTS for releases to the given location
%%%
%%%  usage:
%%%    include_erts.escript target_location
%%% @end

-module(include_erts).
-author("loscher@meta.com").

-export([main/1]).

-mode(compile).

-spec main([string()]) -> ok.
main([TargetPath]) ->
    case filelib:wildcard(filename:join(code:root_dir(), "erts-*")) of
        [ErtsPath] -> ok = copy_dir(ErtsPath, TargetPath);
        Paths -> io:format("expected exactly one erts but found: ~p~n", [Paths])
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("needs exactly one argument: include_erts.escript target_location~n").

copy_dir(From, To) ->
    Cmd = lists:flatten(
        io_lib:format("cp -r ~s ~s", [From, To])
    ),
    io:format("~s~n", [os:cmd(Cmd)]),
    case filelib:is_dir(To) of
        true -> ok;
        false -> erlang:halt(1)
    end.
