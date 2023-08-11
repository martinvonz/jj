%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

%% @doc This module interfaces with the tpx listing protocol, presented in https://www.internalfb.com/code/fbsource/[51311877d966]/fbcode/buck2/docs/test_execution.md?lines=50
%% for high level, see https://www.internalfb.com/code/fbsource/[0101a07bcb98bf8dbed51f55b7b5e4ab8346130f]/fbcode/testinfra/tpx/tpx-buck/src/listing/test_xml.rs?lines=39-55). for
%% code implementation.
-module(listing_interfacer).
-typing([eqwalzier]).

-include_lib("common/include/tpx_records.hrl").
-export([produce_xml_file/2, test_case_constructor/2]).

test_case_to_xml(#test_spec_test_case{suite = Suite, testcases = TestInfos} = _TestCase) ->
    TestElementsXml = lists:map(fun(TestInfo) -> test_info_to_xml(TestInfo) end, TestInfos),
    {testcase, [{suite, Suite}], TestElementsXml}.

-spec test_case_constructor(atom(), [binary()]) -> #test_spec_test_case{}.
test_case_constructor(Suite, Tests) ->
    #test_spec_test_case{
        suite = atom_to_binary(Suite),
        testcases = lists:map(
            fun(TestName) -> #test_spec_test_info{name = TestName, filter = TestName} end, Tests
        )
    }.

test_info_to_xml(#test_spec_test_info{name = TestName, filter = TestName}) ->
    {test, [{name, [TestName]}, {filter, [TestName]}], []}.

-spec produce_xml_file(string(), #test_spec_test_case{}) -> ok.
produce_xml_file(OutputDir, TestCase) ->
    XmlString = xmerl:export_simple([test_case_to_xml(TestCase)], xmerl_xml),
    ok = file:write_file(filename:join(OutputDir, result), XmlString, [append]).
