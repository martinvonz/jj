/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// Generates testmain.go files from a group of test files.
//
// Regular go tests (ones run with `go test`) don't actually define a main
// package. Moreover, Go's reflection does not have the ability to inspect
// packages (e.g. list functions). This script generates a main.go that
// runs some set of tests passed in on the CLI. The code liberally borrows from
// the `go test` implementation at https://github.com/golang/go/blob/master/src/cmd/go/test.go

package main

import (
	"bufio"
	"bytes"
	"errors"
	"flag"
	"fmt"
	"go/ast"
	"go/build"
	"go/doc"
	"go/parser"
	"go/scanner"
	"go/token"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"text/template"
	"unicode"
	"unicode/utf8"
)

// A map of: pkg -> [var name -> file name]
type coverPkgFlag map[string]map[string]string

func (c coverPkgFlag) String() string {
	var buffer bytes.Buffer
	for k, vs := range c {
		if len(vs) > 0 {
			buffer.WriteString(k)
			buffer.WriteString(":")

			first := true
			for f, v := range vs {
				buffer.WriteString(f)
				buffer.WriteString("=")
				buffer.WriteString(v)
				if !first {
					buffer.WriteString(",")
				}
				first = false
			}

			buffer.WriteString(";")
		}
	}
	return buffer.String()
}

func (c coverPkgFlag) Set(value string) error {
	for _, path := range strings.Split(value, ";") {
		pkgAndFiles := strings.Split(path, ":")
		if len(pkgAndFiles) != 2 {
			return errors.New("Bad format: expected path:...;...")
		}
		pkg := pkgAndFiles[0]
		for _, varAndFile := range strings.Split(pkgAndFiles[1], ",") {
			varAndFile := strings.Split(varAndFile, "=")
			if len(varAndFile) != 2 {
				return errors.New("Bad format: expected path:var1=file1,var2=file2,...")
			}

			if c[pkg] == nil {
				c[pkg] = make(map[string]string)
			}

			c[pkg][varAndFile[0]] = varAndFile[1]
		}
	}
	return nil
}

var testCoverMode string
var coverPkgs = make(coverPkgFlag)
var pkgImportPath = flag.String("import-path", "test", "The import path in the test file")
var outputFile = flag.String("output", "", "The path to the output file. Default to stdout.")

var cwd, _ = os.Getwd()
var testCover bool
var testCoverPaths []string

// Resolve argsfiles in args (e.g. `@file.txt`).
func loadArgs(args []string) []string {
	newArgs := make([]string, 0, 0)
	for _, arg := range args {
		if !strings.HasPrefix(arg, "@") {
			newArgs = append(newArgs, arg)
		} else {
			file, _ := os.Open(arg[1:])
			defer file.Close()
			scanner := bufio.NewScanner(file)
			for scanner.Scan() {
				newArgs = append(newArgs, scanner.Text())
			}
		}
	}
	return newArgs
}

func main() {
	os.Args = loadArgs(os.Args)
	flag.Var(coverPkgs, "cover-pkgs", "List of packages & coverage variables to gather coverage info on, in the form of IMPORT-PATH1:var1=file1,var2=file2,var3=file3;IMPORT-PATH2:...")
	flag.StringVar(&testCoverMode, "cover-mode", "", "Cover mode (see `go tool cover`)")
	flag.Parse()

	testFuncs, err := loadTestFuncsFromFiles(*pkgImportPath, flag.Args())
	if err != nil {
		log.Fatalln("Could not read test files:", err)
	}

	for importPath, vars := range coverPkgs {
		cover := coverInfo{&Package{importPath}, make(map[string]*CoverVar)}
		for v, f := range vars {
			cover.Vars[f] = &CoverVar{File: filepath.Join(importPath, f), Var: v}
		}
		testFuncs.Cover = append(testFuncs.Cover, cover)
		testCoverPaths = append(testCoverPaths, importPath)
	}
	testCover = testCoverMode != ""

	out := os.Stdout
	if *outputFile != "" {
		out, err = os.Create(*outputFile)
		if err != nil {
			log.Fatalln("Could not write test main:", err)
		}
	}

	if err := testmainTmpl.Execute(out, testFuncs); err != nil {
		log.Fatalln("Failed to generate main file:", err)
	}
}

func loadTestFuncsFromFiles(packageImportPath string, files []string) (*testFuncs, error) {
	t := &testFuncs{
		Package: &Package{
			ImportPath: packageImportPath,
		},
	}
	for _, filename := range files {
		if err := t.load(filename, "_test", &t.ImportTest, &t.NeedTest); err != nil {
			return nil, err
		}
	}
	return t, nil
}

//
//
//
// Most of the code below is a direct copy from the 'go test' command, but
// adapted to support multiple versions of the go stdlib.  This was last
// updated for the changes in go1.8. The source for the 'go test' generator
// for go1.8 can be found here:
//   https://github.com/golang/go/blob/release-branch.go1.8/src/cmd/go/test.go
//
//
//

// This is a fake version of the actual Package type, since we don't really need all
// ~300 fields of it.
type Package struct {
	ImportPath string `json:",omitempty"` // import path of package in dir
}

// isTestFunc tells whether fn has the type of a testing function. arg
// specifies the parameter type we look for: B, M or T.
func isTestFunc(fn *ast.FuncDecl, arg string) bool {
	if fn.Type.Results != nil && len(fn.Type.Results.List) > 0 ||
		fn.Type.Params.List == nil ||
		len(fn.Type.Params.List) != 1 ||
		len(fn.Type.Params.List[0].Names) > 1 {
		return false
	}
	ptr, ok := fn.Type.Params.List[0].Type.(*ast.StarExpr)
	if !ok {
		return false
	}
	// We can't easily check that the type is *testing.M
	// because we don't know how testing has been imported,
	// but at least check that it's *M or *something.M.
	// Same applies for B and T.
	if name, ok := ptr.X.(*ast.Ident); ok && name.Name == arg {
		return true
	}
	if sel, ok := ptr.X.(*ast.SelectorExpr); ok && sel.Sel.Name == arg {
		return true
	}
	return false
}

// isTest tells whether name looks like a test (or benchmark, according to prefix).
// It is a Test (say) if there is a character after Test that is not a lower-case letter.
// We don't want TesticularCancer.
func isTest(name, prefix string) bool {
	if !strings.HasPrefix(name, prefix) {
		return false
	}
	if len(name) == len(prefix) { // "Test" is ok
		return true
	}
	rune, _ := utf8.DecodeRuneInString(name[len(prefix):])
	return !unicode.IsLower(rune)
}

// shortPath returns an absolute or relative name for path, whatever is shorter.
func shortPath(path string) string {
	if rel, err := filepath.Rel(cwd, path); err == nil && len(rel) < len(path) {
		return rel
	}
	return path
}

// expandScanner expands a scanner.List error into all the errors in the list.
// The default Error method only shows the first error.
func expandScanner(err error) error {
	// Look for parser errors.
	if err, ok := err.(scanner.ErrorList); ok {
		// Prepare error with \n before each message.
		// When printed in something like context: %v
		// this will put the leading file positions each on
		// its own line.  It will also show all the errors
		// instead of just the first, as err.Error does.
		var buf bytes.Buffer
		for _, e := range err {
			e.Pos.Filename = shortPath(e.Pos.Filename)
			buf.WriteString("\n")
			buf.WriteString(e.Error())
		}
		return errors.New(buf.String())
	}
	return err
}

// CoverVar holds the name of the generated coverage variables
// targeting the named file.
type CoverVar struct {
	File string // local file name
	Var  string // name of count struct
}

type coverInfo struct {
	Package *Package
	Vars    map[string]*CoverVar
}

type testFuncs struct {
	Tests       []testFunc
	Benchmarks  []testFunc
	Examples    []testFunc
	TestMain    *testFunc
	Package     *Package
	ImportTest  bool
	NeedTest    bool
	ImportXtest bool
	NeedXtest   bool
	Cover       []coverInfo
}

func (t *testFuncs) CoverMode() string {
	return testCoverMode
}

func (t *testFuncs) CoverEnabled() bool {
	return testCover
}

func (t *testFuncs) ReleaseTag(want string) bool {
	for _, r := range build.Default.ReleaseTags {
		if want == r {
			return true
		}
	}
	return false
}

// ImportPath returns the import path of the package being tested, if it is within GOPATH.
// This is printed by the testing package when running benchmarks.
func (t *testFuncs) ImportPath() string {
	pkg := t.Package.ImportPath
	if strings.HasPrefix(pkg, "_/") {
		return ""
	}
	if pkg == "command-line-arguments" {
		return ""
	}
	return pkg
}

// Covered returns a string describing which packages are being tested for coverage.
// If the covered package is the same as the tested package, it returns the empty string.
// Otherwise it is a comma-separated human-readable list of packages beginning with
// " in", ready for use in the coverage message.
func (t *testFuncs) Covered() string {
	if testCoverPaths == nil {
		return ""
	}
	return " in " + strings.Join(testCoverPaths, ", ")
}

type testFunc struct {
	Package   string // imported package name (_test or _xtest)
	Name      string // function name
	Output    string // output, for examples
	Unordered bool   // output is allowed to be unordered.
}

var testFileSet = token.NewFileSet()

func (t *testFuncs) load(filename, pkg string, doImport, seen *bool) error {
	f, err := parser.ParseFile(testFileSet, filename, nil, parser.ParseComments)
	if err != nil {
		return expandScanner(err)
	}
	for _, d := range f.Decls {
		n, ok := d.(*ast.FuncDecl)
		if !ok {
			continue
		}
		if n.Recv != nil {
			continue
		}
		name := n.Name.String()
		switch {
		case name == "TestMain" && isTestFunc(n, "M"):
			if t.TestMain != nil {
				return errors.New("multiple definitions of TestMain")
			}
			t.TestMain = &testFunc{pkg, name, "", false}
			*doImport, *seen = true, true
		case isTest(name, "Test") && isTestFunc(n, "T"):
			t.Tests = append(t.Tests, testFunc{pkg, name, "", false})
			*doImport, *seen = true, true
		case isTest(name, "Benchmark"):
			err := checkTestFunc(n, "B")
			if err != nil {
				return err
			}
			t.Benchmarks = append(t.Benchmarks, testFunc{pkg, name, "", false})
			*doImport, *seen = true, true
		}
	}
	ex := doc.Examples(f)
	sort.Sort(byOrder(ex))
	for _, e := range ex {
		*doImport = true // import test file whether executed or not
		if e.Output == "" && !e.EmptyOutput {
			// Don't run examples with no output.
			continue
		}

		// Go 1.7 and beyond has support for unordered test output on examples.
		// We can use reflection to see if the Unordered field is there. This
		// can be removed when go 1.6 is not supported by buck.
		unordered := false
		v := reflect.Indirect(reflect.ValueOf(e))
		if f := v.FieldByName("Unordered"); f.IsValid() {
			unordered = f.Bool()
		}

		t.Examples = append(t.Examples, testFunc{pkg, "Example" + e.Name, e.Output, unordered})
		*seen = true
	}
	return nil
}

func checkTestFunc(fn *ast.FuncDecl, arg string) error {
	if !isTestFunc(fn, arg) {
		name := fn.Name.String()
		pos := testFileSet.Position(fn.Pos())
		return fmt.Errorf("%s: wrong signature for %s, must be: func %s(%s *testing.%s)", pos, name, name, strings.ToLower(arg), arg)
	}
	return nil
}

type byOrder []*doc.Example

func (x byOrder) Len() int           { return len(x) }
func (x byOrder) Swap(i, j int)      { x[i], x[j] = x[j], x[i] }
func (x byOrder) Less(i, j int) bool { return x[i].Order < x[j].Order }

var testmainTmpl = template.Must(template.New("main").Parse(`
package main
import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"testing"
{{if .ReleaseTag "go1.8"}}
	internalTestDeps "testing/internal/testdeps"
{{end}}
{{if .ImportTest}}
	{{if .NeedTest}}_test{{else}}_{{end}} {{.Package.ImportPath | printf "%q"}}
{{end}}
{{if .ImportXtest}}
	{{if .NeedXtest}}_xtest{{else}}_{{end}} {{.Package.ImportPath | printf "%s_test" | printf "%q"}}
{{end}}
{{range $i, $p := .Cover}}
	_cover{{$i}} {{$p.Package.ImportPath | printf "%q"}}
{{end}}
)

var tests = []testing.InternalTest{
{{range .Tests}}
	{"{{.Name}}", {{.Package}}.{{.Name}}},
{{end}}
}
var benchmarks = []testing.InternalBenchmark{
{{range .Benchmarks}}
	{"{{.Name}}", {{.Package}}.{{.Name}}},
{{end}}
}
{{if .ReleaseTag "go1.18"}}
var fuzzTargets = []testing.InternalFuzzTarget{
}
{{end}}
var examples = []testing.InternalExample{
{{range .Examples}}
	{"{{.Name}}", {{.Package}}.{{.Name}}, {{.Output | printf "%q"}}{{if $.ReleaseTag "go1.7"}}, {{.Unordered}}{{end}}},
{{end}}
}

var matchPat string
var matchRe *regexp.Regexp

func matchString(pat, str string) (result bool, err error) {
	if matchRe == nil || matchPat != pat {
		matchPat = pat
		matchRe, err = regexp.Compile(matchPat)
		if err != nil {
			return
		}
	}
	return matchRe.MatchString(str), nil
}

{{if .CoverEnabled}}
// Only updated by init functions, so no need for atomicity.
var (
	coverCounters = make(map[string][]uint32)
	coverBlocks = make(map[string][]testing.CoverBlock)
)
func init() {
	{{range $i, $p := .Cover}}
	{{range $file, $cover := $p.Vars}}
	coverRegisterFile({{printf "%q" $cover.File}}, _cover{{$i}}.{{$cover.Var}}.Count[:], _cover{{$i}}.{{$cover.Var}}.Pos[:], _cover{{$i}}.{{$cover.Var}}.NumStmt[:])
	{{end}}
	{{end}}
}
func coverRegisterFile(fileName string, counter []uint32, pos []uint32, numStmts []uint16) {
	if 3*len(counter) != len(pos) || len(counter) != len(numStmts) {
		panic("coverage: mismatched sizes")
	}
	if coverCounters[fileName] != nil {
		// Already registered.
		return
	}
	coverCounters[fileName] = counter
	block := make([]testing.CoverBlock, len(counter))
	for i := range counter {
		block[i] = testing.CoverBlock{
			Line0: pos[3*i+0],
			Col0: uint16(pos[3*i+2]),
			Line1: pos[3*i+1],
			Col1: uint16(pos[3*i+2]>>16),
			Stmts: numStmts[i],
		}
	}
	coverBlocks[fileName] = block
}
{{end}}
func main() {
	// Buck ensures that resources defined on the test targets live in the same
	// directory as the binary. We change the working directory to this
	// directory to make sure that tests can read test fixtures relative to the
	// current working directory. This matches behavior with "go test" from the
	// test author perspective.
	execPath, err := os.Executable()
	if err != nil {
		os.Stderr.WriteString("Unable to get path to test binary executable.")
		os.Exit(1)
	}
	execDir := filepath.Dir(execPath)
	err = os.Chdir(execDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to change directory to %s.", execDir)
		os.Exit(1)
	}
{{if .CoverEnabled}}
	testing.RegisterCover(testing.Cover{
		Mode: {{printf "%q" .CoverMode}},
		Counters: coverCounters,
		Blocks: coverBlocks,
		CoveredPackages: {{printf "%q" .Covered}},
	})
{{end}}
{{if .ReleaseTag "go1.18"}}
	m := testing.MainStart(internalTestDeps.TestDeps{}, tests, benchmarks, fuzzTargets, examples)
{{else if .ReleaseTag "go1.8"}}
	m := testing.MainStart(internalTestDeps.TestDeps{}, tests, benchmarks, examples)
{{else}}
	m := testing.MainStart(matchString, tests, benchmarks, examples)
{{end}}
{{with .TestMain}}
	{{.Package}}.{{.Name}}(m)
{{else}}
	os.Exit(m.Run())
{{end}}
}
`))
