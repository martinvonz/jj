%% #!/usr/bin/env escript
%% -*- erlang -*-
%%! +S 1:1 +sbtu +A1

%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc
%%%  Reads a file containing a mapping from ENV variable to string value
%%%  and outputs a shell file that can be included in other scripts. It
%%%  always adds ERTS_VSN.
%%%
%%%  The purpose is to forward information about the release, e.g. release
%%%  name or version, to a release startup shell script.
%%%
%%%  usage:
%%%    release_variables_builder.escript release_variables
%%% @end

-module(release_variables_builder).
-author("loscher@fb.com").

-export([main/1]).

-mode(compile).

-define(EXITSUCCESS, 0).
-define(EXITERROR, 1).

-spec main([string()]) -> ok.
main([Spec, ReleaseVariablesFile]) ->
    try
        do(Spec, ReleaseVariablesFile),
        erlang:halt(?EXITSUCCESS)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~s:~s~n", [Type, Reason]),
            erlang:halt(?EXITERROR)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("release_variables_builder.escript release_variables").

-spec do(file:filename(), file:filename()) -> ok.
do(Spec, ReleaseVariablesFile) ->
    {ok, [
        #{
            "variables" := Variables
        }
    ]} = file:consult(Spec),
    {ok, F} = file:open(ReleaseVariablesFile, [write]),
    maps:map(
        fun(Key, Value) ->
            ok = file:write(F, io_lib:format("~s=\"~s\"~n", [Key, Value]))
        end,
        Variables#{"ERTS_VSN" => erlang:system_info(version)}
    ),
    ok = file:close(F),
    ok.
