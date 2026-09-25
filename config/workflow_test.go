package config

import (
	"fmt"
	"os"
	"regexp"
	"testing"
)

// The workflow server is C++ and carries its port range in its own source;
// it has to be the range Ports gives it, or the clients dial ports nothing
// listens on.
func TestWorkflowPortsMatch(t *testing.T) {
	src, err := os.ReadFile("../frameworks/workflow/main.cc")
	if err != nil {
		t.Fatal(err)
	}
	port := func(name string) string {
		m := regexp.MustCompile(`(?m)^static const unsigned short ` + name + ` = (\d+);`).FindSubmatch(src)
		if m == nil {
			t.Fatalf("no %v in frameworks/workflow/main.cc", name)
		}
		return string(m[1])
	}
	if got, want := fmt.Sprintf("%v:%v", port("FIRST_PORT"), port("LAST_PORT")), Ports[Workflow]; got != want {
		t.Errorf("frameworks/workflow/main.cc listens on %v, config.Ports[Workflow] is %v", got, want)
	}
}
