package config

import (
	"os"
	"regexp"
	"strconv"
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
