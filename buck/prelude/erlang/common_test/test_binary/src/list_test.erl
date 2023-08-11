%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

-module(list_test).
-compile(warn_missing_spec).

-include_lib("common/include/tpx_records.hrl").

-export([
    list_tests/2,
    list_test_spec/1, list_test_spec/2
]).

%% Fallback oncall
-define(FALLBACK_ONCALL, <<"fallback_oncall">>).

%% match attribute from tree
-define(MATCH_ATTRIBUTE(Attr, Bind),
    {tree, attribute, _, {attribute, {tree, atom, _, Attr}, Bind}}
).
-define(MATCH_LIST(Binds),
    {tree, list, _, {list, Binds, none}}
).
-define(MATCH_STRING(Bind),
    {tree, string, _, Bind}
).

-type group_name() :: atom().
-type test_name() :: atom().
-type suite() :: atom().

%% coming from the output of the group/0 method.
%% See https://www.erlang.org/doc/man/ct_suite.html#Module:groups-0 for the upstream type.
-type groups_output() :: [group_def()].
-type group_def() ::
    {group_name(), properties(), [subgroup_and_test_case()]}
    | {group_name(), [subgroup_and_test_case()]}.
-type subgroup_and_test_case() :: sub_group() | testcase().
-type sub_group() :: group_def() | {group, group_name()}.
-type testcase() :: test_name().

%% coming from the output of the all/0 method.
%% See https://www.erlang.org/doc/man/ct_suite.html#Module:all-0 for the upstream type.
-type all_output() :: [ct_test_def()].
-type ct_test_def() ::
    {group, group_name()}
    | {group, group_name(), properties()}
    | {group, group_name(), properties(), sub_groups_all()}.
-type sub_groups_all() :: [{group_name(), properties()} | {group_name(), properties(), sub_groups_all()}].

-type properties() :: [term()].

%% ------ Public Function --------

%% @doc Outputs a string representation
%% of the tests in the suite, as a XML
%% as defined by tpx-buck2 specifications
%% (see https://www.internalfb.com/code/fbsource/fbcode/buck2/docs/test_execution.md#test-spec-integration-with-tpx)
-spec list_tests(suite(), [module()]) -> #test_spec_test_case{}.
list_tests(Suite, Hooks) ->
    TestNames = list_test_spec(Suite, Hooks),
    listing_interfacer:test_case_constructor(Suite, TestNames).

%% -------------- Internal functions ----------------

%% @doc Test that all the tests in the list are exported.
-spec test_exported_test(suite(), test_name()) -> error | ok.
test_exported_test(Suite, Test) ->
    case erlang:function_exported(Suite, Test, _Arity = 1) of
        false ->
            error(
                {invalid_test,
                    io_lib:format(
                        "The test ~s has been discovered while recursively exploring all/0, " ++
                            "groups/0 but is not an exported method of arity 1",
                        [Test]
                    )}
            );
        true ->
            ok
    end.

-spec load_hooks([module()]) -> ok.
load_hooks(Hooks) ->
    lists:map(fun code:ensure_loaded/1, Hooks),
    ok.

%% We extract the call to the groups() method so that we can type it.
-spec suite_groups(suite(), [module()]) -> groups_output().
suite_groups(Suite, Hooks) ->
    GroupDef =
        case erlang:function_exported(Suite, groups, 0) of
            true -> Suite:groups();
            false -> []
        end,
    lists:foldl(
        fun(Hook, CurrGroupDef) ->
            case erlang:function_exported(Hook, post_groups, 2) of
                true ->
                    Hook:post_groups(Suite, CurrGroupDef);
                false ->
                    CurrGroupDef
            end
        end,
        GroupDef,
        Hooks
    ).

-spec suite_all(suite(), [module()], groups_output) -> all_output().
suite_all(Suite, Hooks, GroupsDef) ->
    TestsDef = Suite:all(),
    lists:foldl(
        fun(Hook, CurrTestsDef) ->
            case erlang:function_exported(Hook, post_all, 3) of
                true ->
                    Hook:post_all(Suite, CurrTestsDef, GroupsDef);
                false ->
                    CurrTestsDef
            end
        end,
        TestsDef,
        Hooks
    ).

-spec list_test([subgroup_and_test_case()], [group_name()], groups_output(), suite()) -> [binary()].
list_test(Node, Groups, SuiteGroups, Suite) ->
    lists:foldl(
        fun
            (Test, ListTestsAcc) when is_atom(Test) ->
                [test_format(Suite, Groups, Test) | ListTestsAcc];
            ({testcase, Test}, ListTestsAcc) when is_atom(Test) ->
                [test_format(Suite, Groups, Test) | ListTestsAcc];
            ({testcase, TestName, _Properties}, ListTestsAcc) when is_atom(TestName) ->
                [test_format(Suite, Groups, TestName) | ListTestsAcc];
            (Group, ListTestsAcc) ->
                lists:append(list_group(Group, Groups, SuiteGroups, Suite), ListTestsAcc)
        end,
        [],
        Node
    ).

%% case where the format of the group is {group, GroupName}, then we need to
%% look for the specifications of the group from the groups() method.
-spec list_group(
    {group, group_name() | [subgroup_and_test_case()]}
    | {group_name(), [subgroup_and_test_case()]}
    | {group_name(), properties(), [subgroup_and_test_case()]},
    [group_name()],
    groups_output(),
    suite()
) ->
    [binary()].
list_group({group, Group}, Groups, SuiteGroups, Suite) when is_atom(Group) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
%% case {group, GroupName, Properties}, similar as above
list_group({group, Group, _}, Groups, SuiteGroups, Suite) when is_atom(Group) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
%% case {group, GroupName, Properties, SubGroupProperties},
%% similar_as_above.
list_group({group, Group, _, _}, Groups, SuiteGroups, Suite) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
%% case {GroupName, SubGroupTests}, then we need to look for the specification of the group
%% from the groups() method as above
list_group({Group, SubGroupTests}, Groups, SuiteGroups, Suite) ->
    Groups1 = lists:append(Groups, [Group]),
    list_test(SubGroupTests, Groups1, SuiteGroups, Suite);
%% case {GroupName, Properties, SubGroupsAndTests},
%% then in this case we explore the SubGroupsAndTests
list_group({Group, _, SubGroupTests}, Groups, SuiteGroups, Suite) ->
    Groups1 = lists:append(Groups, [Group]),
    list_test(SubGroupTests, Groups1, SuiteGroups, Suite).

%% @doc Makes use of the output from the groups/0 method to get the tests and subgroups
%% of the group name given as input
-spec list_sub_group(group_name(), [group_name()], groups_output(), suite()) -> [binary()].
list_sub_group(Group, Groups, SuiteGroups, Suite) when is_list(SuiteGroups) ->
    TestsAndGroups =
        case lists:keyfind(Group, 1, SuiteGroups) of
            {Group, TestsDef} when is_list(TestsDef) -> TestsDef;
            {Group, _, TestsDef} when is_list(TestsDef) -> TestsDef;
            false -> error({invalid_group, Suite, Group});
            GroupSpec -> error({bad_group_spec, GroupSpec})
        end,
    Groups1 = lists:append(Groups, [Group]),
    list_test(TestsAndGroups, Groups1, SuiteGroups, Suite).

%% @doc Given a test that belongs to a common test suite,
%% prints it as follows:
%% name_of_suite.group1:group2:...:groupn.test_name
-spec test_format(suite(), [group_name()], test_name()) -> binary().
test_format(Suite, Groups, Test) ->
    ok = test_exported_test(Suite, Test),
    ListPeriodGroups = lists:join(":", lists:map(fun(Group) -> atom_to_list(Group) end, Groups)),
    GroupString = lists:foldl(
        fun(Element, Acc) -> string:concat(Acc, Element) end,
        "",
        ListPeriodGroups
    ),
    case unicode:characters_to_binary(io_lib:format("~s.~s", [GroupString, Test]), latin1) of
        Error = {'incomplete', _List, _Rest} -> error(Error);
        Error = {'error', _List, _Binary} -> error(Error);
        Binary -> Binary
    end.

-spec list_test_spec(suite()) -> [binary()].
list_test_spec(Suite) ->
    list_test_spec(Suite, []).

%% @doc Creates a Xml representation of all the group / tests
%% of the suite by exploring the suite
-spec list_test_spec(suite(), [module()]) -> [binary()].
list_test_spec(Suite, Hooks) ->
    ok = load_hooks(Hooks),
    _Contacts = get_contacts(Suite),
    GroupsDef = suite_groups(Suite, Hooks),
    AllResult = suite_all(Suite, Hooks, GroupsDef),
    lists:reverse(list_test(AllResult, [], GroupsDef, Suite)).

-spec get_contacts(suite()) -> [binary()].
get_contacts(Suite) ->
    try
        SuiteSource = proplists:get_value(source, Suite:module_info(compile)),
        {ok, Forms} = epp_dodger:parse_file(SuiteSource),
        Oncalls = extract_attribute(oncall, Forms),
        Authors = extract_attribute(author, Forms),
        case lists:append(Oncalls, Authors) of
            [] -> [?FALLBACK_ONCALL];
            Contacts -> Contacts
        end
    catch
        % the suite module is for some reason not accessible
        _:_:_ -> [?FALLBACK_ONCALL]
    end.

extract_attribute(_, []) ->
    [];
extract_attribute(Attribute, [?MATCH_STRING(Data) | Forms]) ->
    [list_to_binary(Data)] ++ extract_attribute(Attribute, Forms);
extract_attribute(Attribute, [?MATCH_LIST(Data) | Forms]) ->
    extract_attribute(Attribute, Data) ++
        extract_attribute(Attribute, Forms);
extract_attribute(Attribute, [?MATCH_ATTRIBUTE(Attribute, Binds) | Forms]) ->
    extract_attribute(Attribute, Binds) ++
        extract_attribute(Attribute, Forms);
extract_attribute(Attribute, [_ | Forms]) ->
    extract_attribute(Attribute, Forms).
