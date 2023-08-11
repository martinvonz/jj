%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

-record(test_spec_test_info, {name :: string(), filter :: string()}).

-record(test_spec_test_case, {suite :: string(), testcases :: [#test_spec_test_info{}]}).

-record(test, {
    name :: string(),
    suite :: string(),
    type :: junit_interfacer:test_result(),
    message :: junit_interfacer:optional(string()),
    stacktrace :: junit_interfacer:optional(string()),
    stdout :: junit_interfacer:optional(string()),
    stderr :: junit_interfacer:optional(string()),
    time :: junit_interfacer:optional(integer())
}).

-record(test_case, {
    name :: string(),
    tests :: [#test{}]
}).
