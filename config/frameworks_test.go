package config

import (
	"sort"
	"testing"
)

// Every framework list is kept in framework-name order - here, in
// script/config.sh and in script/1m_conns_benchmark.sh - so that a framework
// is in the same place in all of them. FrameworkList is also the row order of
// a -sort=framework report, so this is what a diff between two reports lines
// up on.
func TestFrameworkListSortedByName(t *testing.T) {
	if !sort.StringsAreSorted(FrameworkList) {
		t.Errorf("FrameworkList is not in framework-name order: %v", FrameworkList)
	}
}

// Every framework's report row names its language, so every framework needs
// one, and a language with no framework is a leftover.
func TestFrameworkListCoversLangs(t *testing.T) {
	if len(FrameworkList) != len(Langs) {
		t.Errorf("FrameworkList has %d frameworks, Langs has %d", len(FrameworkList), len(Langs))
	}
	for _, framework := range FrameworkList {
		if Langs[framework] == "" {
			t.Errorf("%v has no language", framework)
		}
	}
	if got := FrameworkLang("no-such-framework"); got != "-" {
		t.Errorf("FrameworkLang of an unknown framework = %q, want -", got)
	}
}

// Only Go servers serve /debug/pprof/, so only they are asked for profiles.
func TestHasPprof(t *testing.T) {
	for _, framework := range FrameworkList {
		if got, want := HasPprof(framework), Langs[framework] == "go"; got != want {
			t.Errorf("HasPprof(%v) = %v, want %v", framework, got, want)
		}
	}
	if HasPprof(Axum) {
		t.Errorf("HasPprof(%v) = true, want false", Axum)
	}
	if HasPprof("no-such-framework") {
		t.Errorf("HasPprof of an unknown framework = true, want false")
	}
}

// A framework with no ports is one the clients cannot reach, and a port range
// with no framework is one nothing runs on: the two lists have to carry the
// same names.
func TestFrameworkListCoversPorts(t *testing.T) {
	if len(FrameworkList) != len(Ports) {
		t.Errorf("FrameworkList has %d frameworks, Ports has %d", len(FrameworkList), len(Ports))
	}
	for _, framework := range FrameworkList {
		if _, ok := Ports[framework]; !ok {
			t.Errorf("%v has no port range", framework)
		}
	}
	listed := make(map[string]bool, len(FrameworkList))
	for _, framework := range FrameworkList {
		listed[framework] = true
	}
	for framework := range Ports {
		if !listed[framework] {
			t.Errorf("%v has a port range but is not in FrameworkList", framework)
		}
	}
}
