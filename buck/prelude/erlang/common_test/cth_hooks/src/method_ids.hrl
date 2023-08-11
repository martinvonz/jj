%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

-ifndef(CTH_TPX_METHOD_IDS_HRL).
-define(CTH_TPX_METHOD_IDS, true).

-define(INIT_PER_SUITE, '[init_per_suite]').
-define(INIT_PER_GROUP, '[init_per_group]').
-define(INIT_PER_TESTCASE, '[init_per_testcase]').
-define(END_PER_TESTCASE, '[end_per_testcase]').
-define(END_PER_GROUP, '[end_per_group]').
-define(END_PER_SUITE, '[end_per_suite]').
-define(MAIN_TESTCASE, '[main_testcase]').

-type method_id() ::
    ?INIT_PER_SUITE |
    ?INIT_PER_GROUP |
    ?INIT_PER_TESTCASE |
    ?END_PER_TESTCASE |
    ?END_PER_GROUP |
    ?END_PER_SUITE |
    ?MAIN_TESTCASE.

-endif. % CTH_TPX_METHOD_IDS_HRL
