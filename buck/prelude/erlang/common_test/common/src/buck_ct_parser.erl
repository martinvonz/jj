%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc
%%% Utilities method to parse string args given to the test binary
%%% via user input.
%%% @end

-module(buck_ct_parser).
-compile(warn_missing_spec).

%% Public API
-export([parse_str/1]).

-spec parse_str(string()) -> term().
parse_str("") ->
    [];
parse_str(StrArgs) ->
    {ok, Tokens, _} = erl_scan:string(StrArgs ++ "."),
    {ok, Term} = erl_parse:parse_term(Tokens),
    Term.
