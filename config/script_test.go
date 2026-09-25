package config

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"
)

// script/config.sh's server_ports reads each framework's range out of this
// package's source, for start_server to wait on until the server listens on
// all of it. It has to find every framework's, and the one Ports gives it, or
// a benchmark waits on ports the server never opens.
func TestScriptServerPorts(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	for _, framework := range FrameworkList {
		cmd := exec.Command(bash, "-c", `. ./script/env.sh && server_ports "$1"`, "bash", framework)
		cmd.Dir = ".."
		out, err := cmd.Output()
		if err != nil {
			t.Fatalf("server_ports %v: %v", framework, err)
		}
		if got, want := strings.Replace(strings.TrimSpace(string(out)), " ", ":", 1), Ports[framework]; got != want {
			t.Errorf("server_ports %v = %q, config.Ports[%v] is %q", framework, got, framework, want)
		}
	}
}

// script/config.sh's server_port_range is what docker_benchmark.sh reserves,
// and the README tells a host to reserve, so that no server's port - control
// port included - is handed out as a client's ephemeral one.
func TestScriptServerPortRange(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	first, last := -1, -1
	for _, framework := range FrameworkList {
		ports, err := GetFrameworkBenchmarkPorts(framework)
		if err != nil {
			t.Fatal(err)
		}
		if first < 0 || ports[0] < first {
			first = ports[0]
		}
		if control := ports[len(ports)-1] + 1; control > last {
			last = control
		}
	}
	want := fmt.Sprintf("%d-%d", first, last)

	cmd := exec.Command(bash, "-c", `. ./script/config.sh && server_port_range`)
	cmd.Dir = ".."
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("server_port_range: %v", err)
	}
	if got := strings.TrimSpace(string(out)); got != want {
		t.Errorf("server_port_range = %q, config.Ports covers %q", got, want)
	}

	readme, err := os.ReadFile("../README.md")
	if err != nil {
		t.Fatal(err)
	}
	if line := "sysctl -w net.ipv4.ip_local_reserved_ports=" + want + "\n"; !strings.Contains(string(readme), line) {
		t.Errorf("README.md does not reserve the servers' ports with %q", strings.TrimSpace(line))
	}
}
