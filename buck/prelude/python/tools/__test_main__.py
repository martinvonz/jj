#!/usr/bin/env fbpython -tt
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
This file contains the main module code for buck python test programs.

By default, this is the main module for all python_test() rules.  However,
rules can also specify their own custom main_module.  If you write your own
main module, you can import this module as tools.test.stubs.fbpyunit, to access
any of its code to help implement your main module.
"""

# pyre-unsafe

from __future__ import print_function

import contextlib
import ctypes
import fnmatch
import json
import logging
import optparse
import os
import platform
import re
import sys
import time
import traceback
import unittest
import warnings


with warnings.catch_warnings():
    warnings.filterwarnings("ignore", category=DeprecationWarning)
    import imp

try:
    from StringIO import StringIO  # type: ignore
except ImportError:
    from io import StringIO
try:
    import coverage  # type: ignore
except ImportError:
    coverage = None
try:
    from importlib.machinery import SourceFileLoader
except ImportError:
    SourceFileLoader = None


EXIT_CODE_SUCCESS = 0
EXIT_CODE_TEST_FAILURE = 70


class TestStatus(object):

    ABORTED = "FAILURE"
    PASSED = "SUCCESS"
    FAILED = "FAILURE"
    EXPECTED_FAILURE = "SUCCESS"
    UNEXPECTED_SUCCESS = "FAILURE"
    SKIPPED = "ASSUMPTION_VIOLATION"
    EXCLUDED = "EXCLUDED"


class PathMatcher(object):
    def __init__(self, include_patterns, omit_patterns):
        self.include_patterns = include_patterns
        self.omit_patterns = omit_patterns

    def omit(self, path):
        """
        Omit iff matches any of the omit_patterns or the include patterns are
        not empty and none is matched
        """
        path = os.path.realpath(path)
        return any(fnmatch.fnmatch(path, p) for p in self.omit_patterns) or (
            self.include_patterns
            and not any(fnmatch.fnmatch(path, p) for p in self.include_patterns)
        )

    def include(self, path):
        return not self.omit(path)


class DebugWipeFinder(object):
    """
    PEP 302 finder that uses a DebugWipeLoader for all files which do not need
    coverage
    """

    def __init__(self, matcher):
        self.matcher = matcher

    def find_module(self, fullname, path=None):
        _, _, basename = fullname.rpartition(".")
        try:
            fd, pypath, (_, _, kind) = imp.find_module(basename, path)
        except Exception:
            # Finding without hooks using the imp module failed. One reason
            # could be that there is a zip file on sys.path. The imp module
            # does not support loading from there. Leave finding this module to
            # the others finders in sys.meta_path.
            return None

        if hasattr(fd, "close"):
            fd.close()
        if kind != imp.PY_SOURCE:
            return None
        if self.matcher.include(pypath):
            return None

        """
        This is defined to match CPython's PyVarObject struct
        """

        class PyVarObject(ctypes.Structure):
            _fields_ = [
                ("ob_refcnt", ctypes.c_long),
                ("ob_type", ctypes.c_void_p),
                ("ob_size", ctypes.c_ulong),
            ]

        class DebugWipeLoader(SourceFileLoader):
            """
            PEP302 loader that zeros out debug information before execution
            """

            def get_code(self, fullname):
                code = super(DebugWipeLoader, self).get_code(fullname)
                if code:
                    # Ideally we'd do
                    # code.co_lnotab = b''
                    # But code objects are READONLY. Not to worry though; we'll
                    # directly modify CPython's object
                    code_impl = PyVarObject.from_address(id(code.co_lnotab))
                    code_impl.ob_size = 0
                return code

        return DebugWipeLoader(fullname, pypath)


def optimize_for_coverage(cov, include_patterns, omit_patterns):
    """
    We get better performance if we zero out debug information for files which
    we're not interested in. Only available in CPython 3.3+
    """
    matcher = PathMatcher(include_patterns, omit_patterns)
    if SourceFileLoader and platform.python_implementation() == "CPython":
        sys.meta_path.insert(0, DebugWipeFinder(matcher))


class TeeStream(object):
    def __init__(self, *streams):
        self._streams = streams

    def write(self, data):
        for stream in self._streams:
            stream.write(data)

    def flush(self):
        for stream in self._streams:
            stream.flush()

    def isatty(self):
        return False


class CallbackStream(object):
    def __init__(self, callback, bytes_callback=None, orig=None):
        self._callback = callback
        self._fileno = orig.fileno() if orig else None

        # Python 3 APIs:
        # - `encoding` is a string holding the encoding name
        # - `errors` is a string holding the error-handling mode for encoding
        # - `buffer` should look like an io.BufferedIOBase object

        self.errors = orig.errors if orig else None
        if bytes_callback:
            # those members are only on the io.TextIOWrapper
            self.encoding = orig.encoding if orig else "UTF-8"
            self.buffer = CallbackStream(bytes_callback, orig=orig)

    def write(self, data):
        self._callback(data)

    def flush(self):
        pass

    def isatty(self):
        return False

    def fileno(self):
        return self._fileno


# pyre-fixme[11]: Annotation `unittest._TextTestResult` is not defined as a type.
class BuckTestResult(unittest._TextTestResult):
    """
    Our own TestResult class that outputs data in a format that can be easily
    parsed by buck's test runner.
    """

    def __init__(
        self, stream, descriptions, verbosity, show_output, main_program, suite
    ):
        super(BuckTestResult, self).__init__(stream, descriptions, verbosity)
        self._main_program = main_program
        self._suite = suite
        self._results = []
        self._current_test = None
        self._saved_stdout = sys.stdout
        self._saved_stderr = sys.stderr
        self._show_output = show_output

    def getResults(self):
        return self._results

    def startTest(self, test):
        super(BuckTestResult, self).startTest(test)

        # Pass in the real stdout and stderr filenos.  We can't really do much
        # here to intercept callers who directly operate on these fileno
        # objects.
        sys.stdout = CallbackStream(
            self.addStdout, self.addStdoutBytes, orig=sys.stdout
        )
        sys.stderr = CallbackStream(
            self.addStderr, self.addStderrBytes, orig=sys.stderr
        )
        self._current_test = test
        self._test_start_time = time.time()
        self._current_status = TestStatus.ABORTED
        self._messages = []
        self._stacktrace = None
        self._stdout = ""
        self._stderr = ""

    def _find_next_test(self, suite):
        """
        Find the next test that has not been run.
        """

        for test in suite:

            # We identify test suites by test that are iterable (as is done in
            # the builtin python test harness).  If we see one, recurse on it.
            if hasattr(test, "__iter__"):
                test = self._find_next_test(test)

            # The builtin python test harness sets test references to `None`
            # after they have run, so we know we've found the next test up
            # if it's not `None`.
            if test is not None:
                return test

    def stopTest(self, test):
        sys.stdout = self._saved_stdout
        sys.stderr = self._saved_stderr

        super(BuckTestResult, self).stopTest(test)

        # If a failure occurred during module/class setup, then this "test" may
        # actually be a `_ErrorHolder`, which doesn't contain explicit info
        # about the upcoming test.  Since we really only care about the test
        # name field (i.e. `_testMethodName`), we use that to detect an actual
        # test cases, and fall back to looking the test up from the suite
        # otherwise.
        if not hasattr(test, "_testMethodName"):
            test = self._find_next_test(self._suite)

        self._results.append(
            {
                "testCaseName": "{0}.{1}".format(
                    test.__class__.__module__, test.__class__.__name__
                ),
                "testCase": test._testMethodName,
                "type": self._current_status,
                "time": int((time.time() - self._test_start_time) * 1000),
                "message": os.linesep.join(self._messages),
                "stacktrace": self._stacktrace,
                "stdOut": self._stdout,
                "stdErr": self._stderr,
            }
        )

        self._current_test = None

    def stopTestRun(self):
        cov = self._main_program.get_coverage()
        if cov is not None:
            self._results.append({"coverage": cov})

    @contextlib.contextmanager
    def _withTest(self, test):
        self.startTest(test)
        yield
        self.stopTest(test)

    def _setStatus(self, test, status, message=None, stacktrace=None):
        assert test == self._current_test
        self._current_status = status
        self._stacktrace = stacktrace
        if message is not None:
            if message.endswith(os.linesep):
                message = message[:-1]
            self._messages.append(message)

    def setStatus(self, test, status, message=None, stacktrace=None):
        # addError() may be called outside of a test if one of the shared
        # fixtures (setUpClass/tearDownClass/setUpModule/tearDownModule)
        # throws an error.
        #
        # In this case, create a fake test result to record the error.
        if self._current_test is None:
            with self._withTest(test):
                self._setStatus(test, status, message, stacktrace)
        else:
            self._setStatus(test, status, message, stacktrace)

    def setException(self, test, status, excinfo):
        exctype, value, tb = excinfo
        self.setStatus(
            test,
            status,
            "{0}: {1}".format(exctype.__name__, value),
            "".join(traceback.format_tb(tb)),
        )

    def addSuccess(self, test):
        super(BuckTestResult, self).addSuccess(test)
        self.setStatus(test, TestStatus.PASSED)

    def addError(self, test, err):
        super(BuckTestResult, self).addError(test, err)
        self.setException(test, TestStatus.ABORTED, err)

    def addFailure(self, test, err):
        super(BuckTestResult, self).addFailure(test, err)
        self.setException(test, TestStatus.FAILED, err)

    def addSkip(self, test, reason):
        super(BuckTestResult, self).addSkip(test, reason)
        self.setStatus(test, TestStatus.SKIPPED, "Skipped: %s" % (reason,))

    def addExpectedFailure(self, test, err):
        super(BuckTestResult, self).addExpectedFailure(test, err)
        self.setException(test, TestStatus.EXPECTED_FAILURE, err)

    def addUnexpectedSuccess(self, test):
        super(BuckTestResult, self).addUnexpectedSuccess(test)
        self.setStatus(test, TestStatus.UNEXPECTED_SUCCESS, "Unexpected success")

    def addStdout(self, val):
        self._stdout += val
        if self._show_output:
            self._saved_stdout.write(val)
            self._saved_stdout.flush()

    def addStdoutBytes(self, val):
        string = val.decode("utf-8", errors="backslashreplace")
        self.addStdout(string)

    def addStderr(self, val):
        self._stderr += val
        if self._show_output:
            self._saved_stderr.write(val)
            self._saved_stderr.flush()

    def addStderrBytes(self, val):
        string = val.decode("utf-8", errors="backslashreplace")
        self.addStderr(string)


class BuckTestRunner(unittest.TextTestRunner):
    def __init__(self, main_program, suite, show_output=True, **kwargs):
        super(BuckTestRunner, self).__init__(**kwargs)
        self.show_output = show_output
        self._main_program = main_program
        self._suite = suite

    def _makeResult(self):
        return BuckTestResult(
            self.stream,
            self.descriptions,
            self.verbosity,
            self.show_output,
            self._main_program,
            self._suite,
        )


def _format_test_name(test_class, attrname):
    """
    Format the name of the test buck-style.
    """
    return "{0}.{1}#{2}".format(test_class.__module__, test_class.__name__, attrname)


class StderrLogHandler(logging.StreamHandler):
    """
    This class is very similar to logging.StreamHandler, except that it
    always uses the current sys.stderr object.

    StreamHandler caches the current sys.stderr object when it is constructed.
    This makes it behave poorly in unit tests, which may replace sys.stderr
    with a StringIO buffer during tests.  The StreamHandler will continue using
    the old sys.stderr object instead of the desired StringIO buffer.
    """

    def __init__(self):
        logging.Handler.__init__(self)

    @property
    def stream(self):
        return sys.stderr


class RegexTestLoader(unittest.TestLoader):
    def __init__(self, regex=None):
        self.regex = regex
        super(RegexTestLoader, self).__init__()

    def getTestCaseNames(self, testCaseClass):
        """
        Return a sorted sequence of method names found within testCaseClass
        """

        testFnNames = super(RegexTestLoader, self).getTestCaseNames(testCaseClass)
        if self.regex is None:
            return testFnNames
        robj = re.compile(self.regex)
        matched = []
        for attrname in testFnNames:
            fullname = _format_test_name(testCaseClass, attrname)
            if robj.search(fullname):
                matched.append(attrname)
        return matched


class Loader(object):
    def __init__(self, modules, regex=None):
        self.modules = modules
        self.regex = regex

    def load_all(self):
        loader = RegexTestLoader(self.regex)
        test_suite = unittest.TestSuite()
        for module_name in self.modules:
            __import__(module_name, level=0)
            module = sys.modules[module_name]
            module_suite = loader.loadTestsFromModule(module)
            test_suite.addTest(module_suite)
        return test_suite

    def load_args(self, args):
        loader = RegexTestLoader(self.regex)

        suites = []
        for arg in args:
            suite = loader.loadTestsFromName(arg)
            # loadTestsFromName() can only process names that refer to
            # individual test functions or modules.  It can't process package
            # names.  If there were no module/function matches, check to see if
            # this looks like a package name.
            if suite.countTestCases() != 0:
                suites.append(suite)
                continue

            # Load all modules whose name is <arg>.<something>
            prefix = arg + "."
            for module in self.modules:
                if module.startswith(prefix):
                    suite = loader.loadTestsFromName(module)
                    suites.append(suite)

        return loader.suiteClass(suites)


class MainProgram(object):
    """
    This class implements the main program.  It can be subclassed by
    users who wish to customize some parts of the main program.
    (Adding additional command line options, customizing test loading, etc.)
    """

    DEFAULT_VERBOSITY = 2

    def __init__(self, argv):
        self.init_option_parser()
        self.parse_options(argv)
        self.setup_logging()

    def init_option_parser(self):
        usage = "%prog [options] [TEST] ..."
        op = optparse.OptionParser(usage=usage, add_help_option=False)
        self.option_parser = op

        op.add_option(
            "--hide-output",
            dest="show_output",
            action="store_false",
            default=True,
            help="Suppress data that tests print to stdout/stderr, and only "
            "show it if the test fails.",
        )
        op.add_option(
            "-o",
            "--output",
            help="Write results to a file in a JSON format to be read by Buck",
        )
        op.add_option(
            "-f",
            "--failfast",
            action="store_true",
            default=False,
            help="Stop after the first failure",
        )
        op.add_option(
            "-l",
            "--list-tests",
            action="store_true",
            dest="list",
            default=False,
            help="List tests and exit",
        )
        op.add_option(
            "-L",
            "--list-format",
            dest="list_format",
            choices=["buck", "python"],
            default="python",
            help="List tests format",
        )
        op.add_option(
            "-r",
            "--regex",
            default=None,
            help="Regex to apply to tests, to only run those tests",
        )
        op.add_option(
            "--collect-coverage",
            action="store_true",
            default=False,
            help="Collect test coverage information",
        )
        op.add_option(
            "--coverage-include",
            default="*",
            help='File globs to include in converage (split by ",")',
        )
        op.add_option(
            "--coverage-omit",
            default="",
            help='File globs to omit from converage (split by ",")',
        )
        op.add_option(
            "--logger",
            action="append",
            metavar="<category>=<level>",
            default=[],
            help="Configure log levels for specific logger categories",
        )
        op.add_option(
            "-q",
            "--quiet",
            action="count",
            default=0,
            help="Decrease the verbosity (may be specified multiple times)",
        )
        op.add_option(
            "-v",
            "--verbosity",
            action="count",
            default=self.DEFAULT_VERBOSITY,
            help="Increase the verbosity (may be specified multiple times)",
        )
        op.add_option(
            "-?", "--help", action="help", help="Show this help message and exit"
        )

    def parse_options(self, argv):
        self.options, self.test_args = self.option_parser.parse_args(argv[1:])
        self.options.verbosity -= self.options.quiet

        if self.options.collect_coverage and coverage is None:
            self.option_parser.error("coverage module is not available")
        self.options.coverage_include = self.options.coverage_include.split(",")
        if self.options.coverage_omit == "":
            self.options.coverage_omit = []
        else:
            self.options.coverage_omit = self.options.coverage_omit.split(",")

    def setup_logging(self):
        # Configure the root logger to log at INFO level.
        # This is similar to logging.basicConfig(), but uses our
        # StderrLogHandler instead of a StreamHandler.
        fmt = logging.Formatter("%(pathname)s:%(lineno)s: %(message)s")
        log_handler = StderrLogHandler()
        log_handler.setFormatter(fmt)
        root_logger = logging.getLogger()
        root_logger.addHandler(log_handler)
        root_logger.setLevel(logging.INFO)

        level_names = {
            "debug": logging.DEBUG,
            "info": logging.INFO,
            "warn": logging.WARNING,
            "warning": logging.WARNING,
            "error": logging.ERROR,
            "critical": logging.CRITICAL,
            "fatal": logging.FATAL,
        }

        for value in self.options.logger:
            parts = value.rsplit("=", 1)
            if len(parts) != 2:
                self.option_parser.error(
                    "--logger argument must be of the "
                    "form <name>=<level>: %s" % value
                )
            name = parts[0]
            level_name = parts[1].lower()
            level = level_names.get(level_name)
            if level is None:
                self.option_parser.error(
                    "invalid log level %r for log category %s" % (parts[1], name)
                )
            logging.getLogger(name).setLevel(level)

    def create_loader(self):
        import __test_modules__

        return Loader(__test_modules__.TEST_MODULES, self.options.regex)

    def load_tests(self):
        loader = self.create_loader()
        if self.options.collect_coverage:
            self.start_coverage()
            include = self.options.coverage_include
            omit = self.options.coverage_omit
            if include and "*" not in include:
                optimize_for_coverage(self.cov, include, omit)

        if self.test_args:
            suite = loader.load_args(self.test_args)
        else:
            suite = loader.load_all()
        if self.options.collect_coverage:
            self.cov.start()
        return suite

    def get_tests(self, test_suite):
        tests = []

        for test in test_suite:
            if isinstance(test, unittest.TestSuite):
                tests.extend(self.get_tests(test))
            else:
                tests.append(test)

        return tests

    def run(self):
        test_suite = self.load_tests()

        if self.options.list:
            for test in self.get_tests(test_suite):
                if self.options.list_format == "python":
                    name = str(test)
                elif self.options.list_format == "buck":
                    method_name = getattr(test, "_testMethodName", "")
                    name = _format_test_name(test.__class__, method_name)
                else:
                    raise Exception(
                        "Bad test list format: %s" % (self.options.list_format,)
                    )

                print(name)
            return EXIT_CODE_SUCCESS
        else:
            result = self.run_tests(test_suite)
            if self.options.output is not None:
                with open(self.options.output, "w") as f:
                    json.dump(result.getResults(), f, indent=4, sort_keys=True)
            if not result.wasSuccessful():
                return EXIT_CODE_TEST_FAILURE
            return EXIT_CODE_SUCCESS

    def run_tests(self, test_suite):
        # Install a signal handler to catch Ctrl-C and display the results
        # (but only if running >2.6).
        if sys.version_info[0] > 2 or sys.version_info[1] > 6:
            unittest.installHandler()

        # Run the tests
        runner = BuckTestRunner(
            self,
            test_suite,
            verbosity=self.options.verbosity,
            show_output=self.options.show_output,
        )
        result = runner.run(test_suite)

        if self.options.collect_coverage and self.options.show_output:
            self.cov.stop()
            if self.cov.html_report:
                self.cov.html_report()
            else:
                self.cov.report(file=sys.stdout)

        return result

    def start_coverage(self):
        if not self.options.collect_coverage:
            return

        # Keep the original working dir in case tests use os.chdir
        self._original_working_dir = os.getcwd()

        self.cov = coverage.Coverage(
            include=self.options.coverage_include, omit=self.options.coverage_omit
        )
        self.cov.erase()
        self.cov.start()

    def get_coverage(self):
        if not self.options.collect_coverage:
            return None
        result = {}

        # Switch back to the original working directory.
        os.chdir(self._original_working_dir)

        self.cov.stop()

        try:
            f = StringIO()
            self.cov.report(file=f)
            lines = f.getvalue().split("\n")
        except coverage.misc.CoverageException:
            # Nothing was covered. That's fine by us
            return result

        # N.B.: the format of the coverage library's output differs
        # depending on whether one or more files are in the results
        for line in lines[2:]:
            if line.strip("-") == "":
                break
            r = line.split()[0]
            analysis = self.cov.analysis2(r)
            covString = self.convert_to_diff_cov_str(analysis)
            if covString:
                result[r] = covString

        return result

    def convert_to_diff_cov_str(self, analysis):
        # Info on the format of analysis:
        # http://nedbatchelder.com/code/coverage/api.html
        if not analysis:
            return None
        numLines = max(
            analysis[1][-1] if len(analysis[1]) else 0,
            analysis[2][-1] if len(analysis[2]) else 0,
            analysis[3][-1] if len(analysis[3]) else 0,
        )
        lines = ["N"] * numLines
        for l in analysis[1]:
            lines[l - 1] = "C"
        for l in analysis[2]:
            lines[l - 1] = "X"
        for l in analysis[3]:
            lines[l - 1] = "U"
        return "".join(lines)


def main(argv):
    return MainProgram(sys.argv).run()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
