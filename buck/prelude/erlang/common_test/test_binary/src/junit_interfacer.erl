%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

%% @doc This module interfaces with the tpx test protocol, that is based on junit.
%% See https://www.internalfb.com/code/fbsource/[51311877d966]/fbcode/buck2/docs/test_execution.md?lines=53
%% for tpx documentation, and https://www.internalfb.com/code/fbsource/%5B20c96bf58ffecff08a87b89035518b392985308b%5D/fbcode/testinfra/tpx/tpx-output/src/buck_junitlike_xml.rs
%% for the details about the test protocol.
-module(junit_interfacer).

-include_lib("common/include/tpx_records.hrl").

-export([test_case_to_xml/1, write_xml_output/5]).

-export_type([test_result/0, optional/1]).

-type test_result() :: success | failure | assumption_violation | disabled | excluded | dry_run.
-type case_result() :: cth_tpx_test_tree:case_result().

-type optional(Type) :: undefined | Type.

% cth_tpx outcome are skipped failed passed omitted
outcome_to_result(failed) -> failure;
outcome_to_result(passed) -> success;
outcome_to_result(omitted) -> excluded;
outcome_to_result(skipped) -> failure.

% %% See https://www.internalfb.com/code/fbsource/[20c96bf58ffecff08a87b89035518b392985308b]/fbcode/testinfra/tpx/tpx-output/src/buck_junitlike_xml.rs?lines=73%2C80
format_result(success) -> "SUCCESS";
format_result(failure) -> "FAILURE";
format_result(assumption_violation) -> "ASSUMPTIONVIOLATION";
format_result(disabled) -> "DISABLED";
format_result(excluded) -> "EXCLUDED";
format_result(dry_run) -> "DRYRUN".

-spec write_xml_output(string(), [case_result()], atom(), any(), binary()) -> {ok, file:filename()}.
write_xml_output(OutputDir, TpxResults, Suite, Exit, Stdout) ->
    TestCase = results_to_test_case(TpxResults, Suite, Exit, Stdout),
    Export = xmerl:export_simple([test_case_to_xml(TestCase)], xmerl_xml),
    {ok, OutputFile} = filename:join(OutputDir, "results.xml"),
    file:write_file(OutputFile, Export),
    {ok, OutputFile}.

test_case_to_xml(#test_case{
    name = Name,
    tests = Tests
}) ->
    {testcase, [{name, Name}], lists:map(fun test_to_xml/1, Tests)}.

test_to_xml(
    #test{} = Test
) ->
    Fields = record_info(fields, test),
    [test | Values] = tuple_to_list(Test),
    ValuePairs = lists:zip(Fields, Values),
    TestRoughXml = lists:filter(fun({_Key, Value}) -> Value =/= undefined end, ValuePairs),
    TestXml = lists:map(
        fun
            ({type, Value}) -> {type, format_result(Value)};
            ({time, Time}) -> {time, float_to_list(Time)};
            ({Key, Value}) -> {Key, Value}
        end,
        TestRoughXml
    ),
    {test, TestXml, []}.

% Xml format according to https://www.internalfb.com/code/fbsource/[28cd5933c399]/fbcode/buck2/docs/test_execution.md?lines=98
%% To be encoded following https://erlang.org/doc/apps/xmerl/xmerl_ug.html
-spec method_result_to_test_info(cth_tpx_test_tree:method_result(), atom(), any(), binary()) -> #test{}.
method_result_to_test_info(MethodResult, Suite, Exit, StdOut) ->
    #{
        main := #{
            name := Name,
            details := Details,
            startedTime := Start,
            endedTime := End,
            outcome := Outcome
        }
    } = MethodResult,
    Details1 = unicode:characters_to_list(Details),
    #test{
        suite = atom_to_list(Suite),
        name = Name,
        type = format_result(outcome_to_result(Outcome)),
        time = End - Start,
        message = Details1,
        stdout = unicode:characters_to_list(
            io_lib:format("ct exitted with Status ~p, ~n~s", [Exit, StdOut])
        )
    }.

-spec results_to_test_case([case_result()], atom(), any(), binary()) -> #test_case{}.
results_to_test_case(ListsResults, Suite, Exit, StdOut) ->
    TestElements = lists:map(
        fun(Test) ->
            method_result_to_test_info(Test, Suite, Exit, StdOut)
        end,
        ListsResults
    ),
    #test_case{name = atom_to_list(Suite), tests = TestElements}.
