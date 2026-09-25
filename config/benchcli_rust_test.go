package config

import (
	"os"
	"regexp"
	"strconv"
	"strings"
	"testing"
)

// benchcli-rust cannot import this package, so it carries FrameworkList,
// Langs and Ports as one table in its own source. It has to be this
// package's, row for row, or it dials ports nothing listens on and labels
// its reports with the wrong language.
func TestRustClientFrameworksMatch(t *testing.T) {
	src, err := os.ReadFile("../benchcli-rust/src/config.rs")
	if err != nil {
		t.Fatal(err)
	}
	rows := regexp.MustCompile(`\("([a-z0-9_]+)", "([a-z+]+)", (\d+), (\d+)\),`).FindAllSubmatch(src, -1)
	if len(rows) != len(FrameworkList) {
		t.Fatalf("benchcli-rust/src/config.rs lists %d frameworks, FrameworkList %d", len(rows), len(FrameworkList))
	}
	for i, row := range rows {
		name, lang, first, last := string(row[1]), string(row[2]), string(row[3]), string(row[4])
		if name != FrameworkList[i] {
			t.Errorf("row %d is %v, FrameworkList has %v", i, name, FrameworkList[i])
			continue
		}
		if lang != Langs[name] {
			t.Errorf("%v: lang %v, Langs has %v", name, lang, Langs[name])
		}
		ports, _ := GetFrameworkBenchmarkPorts(name)
		if first != strconv.Itoa(ports[0]) || last != strconv.Itoa(ports[len(ports)-1]) {
			t.Errorf("%v: ports %v:%v, Ports has %v", name, first, last, Ports[name])
		}
	}
}

// benchcli-rust skips BenchPipeline for the frameworks in its NO_PIPELINE,
// which has to be NoPipeline, or the two clients disagree on which rows of
// the table are measured.
func TestRustClientNoPipelineMatches(t *testing.T) {
	src, err := os.ReadFile("../benchcli-rust/src/config.rs")
	if err != nil {
		t.Fatal(err)
	}
	list := regexp.MustCompile(`(?s)pub const NO_PIPELINE: &\[&str\] = &\[(.*?)\];`).FindSubmatch(src)
	if list == nil {
		t.Fatal("no NO_PIPELINE in benchcli-rust/src/config.rs")
	}
	var got []string
	for _, m := range regexp.MustCompile(`"([a-z0-9_]+)"`).FindAllSubmatch(list[1], -1) {
		got = append(got, string(m[1]))
	}
	if strings.Join(got, ",") != strings.Join(NoPipeline, ",") {
		t.Errorf("benchcli-rust/src/config.rs NO_PIPELINE is %v, NoPipeline is %v", got, NoPipeline)
	}
}
