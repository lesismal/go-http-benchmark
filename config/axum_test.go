package config

import (
	"fmt"
	"os"
	"regexp"
	"testing"
)

// The axum server is Rust and carries its port range in its own source; it
// has to be the range Ports gives it, or the clients dial ports nothing
// listens on.
func TestAxumPortsMatch(t *testing.T) {
	src, err := os.ReadFile("../frameworks/axum/src/main.rs")
	if err != nil {
		t.Fatal(err)
	}
	port := func(name string) string {
		m := regexp.MustCompile(`(?m)^const ` + name + `: u16 = (\d+);`).FindSubmatch(src)
		if m == nil {
			t.Fatalf("no const %v in frameworks/axum/src/main.rs", name)
		}
		return string(m[1])
	}
	if got, want := fmt.Sprintf("%v:%v", port("FIRST_PORT"), port("LAST_PORT")), Ports[Axum]; got != want {
		t.Errorf("frameworks/axum/src/main.rs listens on %v, config.Ports[Axum] is %v", got, want)
	}
}
