package report

import (
	"os"
	"reflect"
	"regexp"
	"testing"
)

// benchcli-rust writes the report files this package reads, so its structs
// have to carry every JSON name the ones here do, in the same order - an
// unknown name is dropped by encoding/json and a missing one reads as zero,
// both without a word.
func TestRustClientWritesEveryField(t *testing.T) {
	src, err := os.ReadFile("../../benchcli-rust/src/report.rs")
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range []interface{}{ConnectionsReport{}, BenchEchoReport{}, BenchRateReport{}} {
		typ := reflect.TypeOf(r)
		var want []string
		for i := 0; i < typ.NumField(); i++ {
			if name := typ.Field(i).Tag.Get("json"); name != "-" && name != "" {
				want = append(want, name)
			}
		}
		block := regexp.MustCompile(`(?s)pub struct ` + typ.Name() + ` \{(.*?)\n\}`).FindSubmatch(src)
		if block == nil {
			t.Errorf("no struct %v in benchcli-rust/src/report.rs", typ.Name())
			continue
		}
		var got []string
		for _, m := range regexp.MustCompile(`rename = "([A-Za-z0-9]+)"`).FindAllSubmatch(block[1], -1) {
			got = append(got, string(m[1]))
		}
		if !equal(got, want) {
			t.Errorf("%v: benchcli-rust writes %v, want %v", typ.Name(), got, want)
		}
	}
}
