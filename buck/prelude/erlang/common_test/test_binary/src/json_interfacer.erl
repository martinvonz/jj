%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

-module(json_interfacer).
% Methods used for serialization to the type
% defined in tpx in
% https://www.internalfb.com/code/fbsource/[bb9e81daacad]/fbcode/testinfra/tpx/tpx-output/src/erl_parser.rs
%
%
%

-export([write_json_output/2, format_json/1, status_name/1]).

-define(PASSED, <<"PASSED">>).
-define(FAILED, <<"FAILED">>).
-define(SKIPPED, <<"SKIPPED">>).
-define(TIMEOUT, <<"TIMEOUT">>).
-define(OMITTED, <<"OMITTED">>).

status(passed) -> 1;
status(failed) -> 2;
status(skipped) -> 3;
status(timeout) -> 5;
status(omitted) -> 7.

status_name(1) -> passed;
status_name(2) -> failed;
status_name(3) -> skipped;
status_name(5) -> timeout;
status_name(7) -> omitted.

summary(passed) -> ?PASSED;
summary(failed) -> ?FAILED;
summary(skipped) -> ?SKIPPED;
summary(timeout) -> ?TIMEOUT;
summary(omitted) -> ?OMITTED.

-type formatted_result() ::
    #{
        name := binary(),
        status := binary(),
        summary := integer(),
        details := binary()
    }
    | #{
        name := binary(),
        status := binary(),
        summary := integer(),
        details := binary(),
        endedTime := float(),
        durationSecs := float()
    }.

-type case_result() :: cth_tpx_test_tree:case_result().

-type formatted_case_result() ::
    #{
        inits := [formatted_result()],
        main := formatted_result(),
        ends := [formatted_result()]
    }.

-spec write_json_output(string(), [case_result()]) -> {ok, file:filename()}.
write_json_output(OutputDir, TpxResults) ->
    OuptputFile = filename:join(OutputDir, "result_exec.json"),
    file:write_file(OuptputFile, format_json(TpxResults)),
    {ok, OuptputFile}.

-spec format_json([case_result()]) -> string().
format_json(TpxResults) ->
    jsone:encode(lists:map(fun(CaseResult) -> format_case(CaseResult) end, TpxResults)).

-spec format_case([case_result()]) -> [formatted_case_result()].
format_case(
    #{
        inits := Inits,
        main := Main,
        ends := Ends
    } = _CaseResult
) ->
    #{
        inits => lists:map(fun(MethodResult) -> format_method_result(MethodResult) end, Inits),
        main => format_method_result(Main),
        ends => lists:map(fun(MethodResult) -> format_method_result(MethodResult) end, Ends)
    }.

-spec format_method_result(cth_tpx_test_tree:method_result()) -> formatted_result().

format_method_result(
    #{
        name := Name,
        startedTime := Start,
        endedTime := End,
        outcome := Outcome,
        details := Details,
        std_out := StdOut
    } = _TestResult
) ->
    #{
        name => list_to_binary(Name),
        endedTime => trunc(End),
        durationSecs => End - Start,
        status => status(Outcome),
        summary => summary(Outcome),
        details => unicode:characters_to_binary(Details),
        std_out => unicode:characters_to_binary(StdOut)
    };
format_method_result(
    #{
        name := Name,
        outcome := Outcome,
        details := Details,
        std_out := StdOut
    } = _TestResult
) ->
    #{
        name => list_to_binary(Name),
        status => status(Outcome),
        summary => summary(Outcome),
        details => unicode:characters_to_binary(Details),
        std_out => unicode:characters_to_binary(StdOut),
        durationSecs => 0.0
    }.
