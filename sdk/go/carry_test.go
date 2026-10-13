package carry

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

var ctx = context.Background()

func setup(t *testing.T) string {
	t.Helper()
	if os.Getenv("CARRY_BIN") == "" {
		abs, _ := filepath.Abs("../testdata/fake-carry")
		t.Setenv("CARRY_BIN", abs)
	}
	db := filepath.Join(t.TempDir(), "db.json")
	t.Setenv("FAKE_CARRY_DB", db)
	return db
}

func lastArgv(t *testing.T, db string) string {
	b, _ := os.ReadFile(db)
	var d struct {
		LastCall struct{ Argv []string } `json:"last_call"`
	}
	_ = json.Unmarshal(b, &d)
	return strings.Join(d.LastCall.Argv, " ")
}

func TestPutGet(t *testing.T) {
	db := setup(t)
	c := New(Options{Profile: "dev"})
	if _, err := c.Put(ctx, "web-1:/etc/app.yaml", []byte("x: 1"), 0); err != nil {
		t.Fatal(err)
	}
	got, err := c.Get(ctx, "web-1:/etc/app.yaml")
	if err != nil || string(got) != "x: 1" {
		t.Fatalf("got %q %v", got, err)
	}
	if !strings.Contains(lastArgv(t, db), "--profile dev") {
		t.Fatal("profile not passed")
	}
}

func TestBinaryAndMode(t *testing.T) {
	setup(t)
	c := New(Options{})
	c.Put(ctx, "s:/bin", []byte{0, 255, 1}, 0o600)
	got, _ := c.Get(ctx, "s:/bin")
	ls, _ := c.Ls(ctx, "s")
	if !reflect.DeepEqual(got, []byte{0, 255, 1}) || ls.Entries[0].Mode != 0o600 {
		t.Fatalf("%v %+v", got, ls)
	}
}

func TestFileOps(t *testing.T) {
	setup(t)
	c := New(Options{})
	c.Put(ctx, "s:/a", []byte("1"), 0)
	if _, err := c.Mv(ctx, "s:/a", "/b"); err != nil {
		t.Fatal(err)
	}
	ls, _ := c.Ls(ctx, "s")
	if len(ls.Entries) != 1 || ls.Entries[0].Path != "/b" {
		t.Fatalf("%+v", ls)
	}
	c.Rm(ctx, "s:/b")
	lg, _ := c.Log(ctx, "s", 2)
	if len(lg.Versions) != 2 {
		t.Fatalf("%+v", lg)
	}
	rb, _ := c.Rollback(ctx, "s", 1)
	if rb.RestoredFrom != 1 {
		t.Fatal(rb)
	}
	if _, err := c.Diff(ctx, "s", 1, 2); err != nil {
		t.Fatal(err)
	}
	st, _ := c.States(ctx)
	if !reflect.DeepEqual(st, []StateInfo{{"s", 4}}) {
		t.Fatal(st)
	}
}

func TestEnvNotInArgv(t *testing.T) {
	db := setup(t)
	c := New(Options{})
	if _, err := c.EnvSet(ctx, "s", map[string]string{"DB_PASSWORD": "hunter2", "MODE": "prod"}); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(lastArgv(t, db), "hunter2") {
		t.Fatal("secret leaked into argv")
	}
	env, _ := c.EnvGet(ctx, "s", "MODE")
	if !reflect.DeepEqual(env, map[string]string{"MODE": "prod"}) {
		t.Fatal(env)
	}
	c.EnvUnset(ctx, "s", "MODE")
	env, _ = c.EnvGet(ctx, "s", "")
	if !reflect.DeepEqual(env, map[string]string{"DB_PASSWORD": "hunter2"}) {
		t.Fatal(env)
	}
}

func TestToolAndStatus(t *testing.T) {
	setup(t)
	c := New(Options{Env: map[string]string{"CARRY_STATE": "s"}})
	if _, err := c.ToolAdd(ctx, "s", "jq@1.7", ToolAddOptions{Channel: "stable"}); err != nil {
		t.Fatal(err)
	}
	st, err := c.Status(ctx)
	if err != nil || *st.State != "s" || len(st.Tools) != 1 || st.Tools[0].Name != "jq" {
		t.Fatalf("%+v %v", st, err)
	}
}

func TestErrors(t *testing.T) {
	setup(t)
	c := New(Options{})
	cases := []struct {
		err  error
		kind error
		code string
		exit int
	}{
		{func() error { _, e := c.Put(ctx, "conflict:/a", nil, 0); return e }(), ErrConflict, "conflict", 5},
		{func() error { _, e := c.Ls(ctx, "unreachable"); return e }(), ErrUnreachable, "unreachable", 3},
		{func() error { _, e := c.Ls(ctx, "badkey"); return e }(), ErrRefused, "bad_key", 4},
		{func() error { _, e := c.Get(ctx, "s:/missing"); return e }(), ErrUsage, "not_found", 2},
	}
	for _, tc := range cases {
		var ce *Error
		if !errors.Is(tc.err, tc.kind) || !errors.As(tc.err, &ce) || ce.Code != tc.code || ce.ExitCode != tc.exit {
			t.Errorf("got %v, want %v/%s/%d", tc.err, tc.kind, tc.code, tc.exit)
		}
	}
	_, err := New(Options{Bin: "/nonexistent/carry"}).States(ctx)
	if !errors.Is(err, ErrCarry) {
		t.Fatal(err)
	}
}

func TestRun(t *testing.T) {
	setup(t)
	raw, err := New(Options{}).Run(ctx, []string{"states"}, nil)
	if err != nil || string(raw) != `{"states": []}` {
		t.Fatalf("%s %v", raw, err)
	}
}
