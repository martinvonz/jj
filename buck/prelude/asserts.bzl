# Copyright 2017 The Bazel Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# @lint-ignore-every LICENSELINT

"""Testing support.

This is a modified version of https://github.com/bazelbuild/bazel-skylib/blob/main/lib/unittest.bzl.
Currently, if there are any failures, these are raised immediately by calling fail(),
which trigger an analysis-time build error.
"""

def _assert_equals(expected, actual, msg = None):
    """Asserts that the given `expected` and `actual` are equal.

    Args:
      expected: the expected value of some computation.
      actual: the actual value return by some computation.
      msg: An optional message that will be printed that describes the failure.
        If omitted, a default will be used.
    """
    if expected != actual:
        expectation_msg = 'Expected "%s", but got "%s"' % (expected, actual)
        if msg:
            full_msg = "%s (%s)" % (msg, expectation_msg)
        else:
            full_msg = expectation_msg
        fail(full_msg)

def _assert_true(
        condition,
        msg = "Expected condition to be true, but was false."):
    """Asserts that the given `condition` is true.

    Args:
      condition: A value that will be evaluated in a Boolean context.
      msg: An optional message that will be printed that describes the failure.
        If omitted, a default will be used.
    """
    if not condition:
        fail(msg)

def _assert_false(
        condition,
        msg = "Expected condition to be false, but was true."):
    """Asserts that the given `condition` is false.

    Args:
      condition: A value that will be evaluated in a Boolean context.
      msg: An optional message that will be printed that describes the failure.
        If omitted, a default will be used.
    """
    if condition:
        fail(msg)

asserts = struct(
    equals = _assert_equals,
    true = _assert_true,
    false = _assert_false,
)
