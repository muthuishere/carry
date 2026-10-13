// Package carry is a thin wrapper over the carry CLI JSON interface
// (docs/cli-contract.md). It runs the binary with --json and maps exit
// codes / error.code to typed errors.
package carry

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"sort"
	"strconv"
	"strings"
)

// Error is returned for any failed carry invocation. Use errors.Is with the
// sentinel kinds (ErrUsage, ErrUnreachable, ErrRefused, ErrConflict).
type Error struct {
	Code     string
	Message  string
	ExitCode int
	Kind     error
}

func (e *Error) Error() string { return fmt.Sprintf("carry: %s: %s (exit %d)", e.Code, e.Message, e.ExitCode) }
func (e *Error) Unwrap() error { return e.Kind }

var (
	ErrCarry       = errors.New("carry error")
	ErrUsage       = errors.New("usage error")
	ErrUnreachable = errors.New("store unreachable")
	ErrRefused     = errors.New("store refused")
	ErrConflict    = errors.New("conflict")
)

var byCode = map[string]error{
	"usage": ErrUsage, "readonly": ErrUsage, "not_found": ErrUsage,
	"unreachable": ErrUnreachable,
	"refused":     ErrRefused, "invalid": ErrRefused, "bad_key": ErrRefused,
	"conflict": ErrConflict,
}
var byExit = map[int]error{2: ErrUsage, 3: ErrUnreachable, 4: ErrRefused, 5: ErrConflict}

func newError(code, msg string, exit int) *Error {
	kind, ok := byCode[code]
	if !ok {
		if kind, ok = byExit[exit]; !ok {
			kind = ErrCarry
		}
	}
	return &Error{Code: code, Message: msg, ExitCode: exit, Kind: kind}
}

// Options configure a Client. Zero values mean "not set".
type Options struct {
	Bin      string // default $CARRY_BIN, then "carry" on PATH
	Profile  string
	Repo     string
	Endpoint string
	Timeout  int               // seconds, passed as --timeout
	Env      map[string]string // extra env for the child; put secrets here
}

type Client struct{ opts Options }

func New(opts Options) *Client {
	if opts.Bin == "" {
		opts.Bin = os.Getenv("CARRY_BIN")
	}
	if opts.Bin == "" {
		opts.Bin = "carry"
	}
	return &Client{opts: opts}
}

func (c *Client) globalArgs() []string {
	a := []string{"--json"}
	if c.opts.Profile != "" {
		a = append(a, "--profile", c.opts.Profile)
	}
	if c.opts.Repo != "" {
		a = append(a, "--repo", c.opts.Repo)
	}
	if c.opts.Endpoint != "" {
		a = append(a, "--endpoint", c.opts.Endpoint)
	}
	if c.opts.Timeout > 0 {
		a = append(a, "--timeout", strconv.Itoa(c.opts.Timeout))
	}
	return a
}

// Run executes any carry command with --json and returns the raw result.
func (c *Client) Run(ctx context.Context, args []string, stdin []byte) (json.RawMessage, error) {
	cmd := exec.CommandContext(ctx, c.opts.Bin, append(c.globalArgs(), args...)...)
	cmd.Env = os.Environ()
	for k, v := range c.opts.Env {
		cmd.Env = append(cmd.Env, k+"="+v)
	}
	cmd.Stdin = bytes.NewReader(stdin)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	runErr := cmd.Run()
	exit := 0
	var ee *exec.ExitError
	if errors.As(runErr, &ee) {
		exit = ee.ExitCode()
	} else if runErr != nil {
		return nil, newError("internal", runErr.Error(), 1)
	}
	var env struct {
		OK     bool            `json:"ok"`
		Result json.RawMessage `json:"result"`
		Error  *struct {
			Code    string `json:"code"`
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal(stdout.Bytes(), &env); err != nil {
		return nil, newError("internal", "unparseable output: "+strings.TrimSpace(stderr.String()), exit)
	}
	if env.OK {
		return env.Result, nil
	}
	if env.Error == nil {
		return nil, newError("internal", "unknown error", exit)
	}
	return nil, newError(env.Error.Code, env.Error.Message, exit)
}

func call[T any](c *Client, ctx context.Context, stdin []byte, args ...string) (T, error) {
	var out T
	raw, err := c.Run(ctx, args, stdin)
	if err != nil {
		return out, err
	}
	err = json.Unmarshal(raw, &out)
	return out, err
}

type PutResult struct {
	State    string `json:"state"`
	Path     string `json:"path"`
	Version  int    `json:"version"`
	Size     int    `json:"size"`
	Uploaded bool   `json:"uploaded"`
}
type Entry struct {
	Path string `json:"path"`
	Kind string `json:"kind"`
	Size int    `json:"size"`
	Mode int    `json:"mode"`
}
type LsResult struct {
	State   string  `json:"state"`
	Version int     `json:"version"`
	Entries []Entry `json:"entries"`
}
type RmResult struct {
	State   string `json:"state"`
	Path    string `json:"path"`
	Version int    `json:"version"`
}
type MvResult struct {
	State   string `json:"state"`
	From    string `json:"from"`
	To      string `json:"to"`
	Version int    `json:"version"`
}
type LogEntry struct {
	Version int      `json:"version"`
	Op      string   `json:"op"`
	Args    []string `json:"args"`
	At      string   `json:"at"`
	By      string   `json:"by"`
}
type LogResult struct {
	State    string     `json:"state"`
	Versions []LogEntry `json:"versions"`
}
type DiffResult struct {
	Added       []string `json:"added"`
	Removed     []string `json:"removed"`
	Changed     []string `json:"changed"`
	EnvChanged  []string `json:"env_changed"`
	AppsChanged []string `json:"apps_changed"`
}
type RollbackResult struct {
	State        string `json:"state"`
	Version      int    `json:"version"`
	RestoredFrom int    `json:"restored_from"`
}
type EnvResult struct {
	State   string   `json:"state"`
	Version int      `json:"version"`
	Keys    []string `json:"keys"`
}
type ToolResult struct {
	State   string `json:"state"`
	Version int    `json:"version"`
	Tool    string `json:"tool"`
}
type ToolStatus struct {
	Name   string `json:"name"`
	Status string `json:"status"`
}
type StatusResult struct {
	State         *string      `json:"state"`
	Mode          string       `json:"mode"`
	Version       *int         `json:"version"`
	Root          string       `json:"root"`
	LastRestoreMs *int         `json:"last_restore_ms"`
	Error         *string      `json:"error"`
	Tools         []ToolStatus `json:"tools"`
}
type StateInfo struct {
	Name    string `json:"name"`
	Version int    `json:"version"`
}

// Put uploads data to "<state>:<path>". mode<=0 leaves the default.
func (c *Client) Put(ctx context.Context, path string, data []byte, mode int) (PutResult, error) {
	args := []string{"put", path, "-"}
	if mode > 0 {
		args = append(args, "--mode", strconv.FormatInt(int64(mode), 8))
	}
	return call[PutResult](c, ctx, data, args...)
}

func (c *Client) Get(ctx context.Context, path string) ([]byte, error) {
	r, err := call[struct {
		ContentBase64 string `json:"content_base64"`
	}](c, ctx, nil, "get", path)
	if err != nil {
		return nil, err
	}
	return base64.StdEncoding.DecodeString(r.ContentBase64)
}

func (c *Client) Ls(ctx context.Context, target string) (LsResult, error) {
	return call[LsResult](c, ctx, nil, "ls", target)
}
func (c *Client) Rm(ctx context.Context, path string) (RmResult, error) {
	return call[RmResult](c, ctx, nil, "rm", path)
}
func (c *Client) Mv(ctx context.Context, from, to string) (MvResult, error) {
	return call[MvResult](c, ctx, nil, "mv", from, to)
}

// Log returns history; limit<=0 means no limit.
func (c *Client) Log(ctx context.Context, state string, limit int) (LogResult, error) {
	if limit > 0 {
		return call[LogResult](c, ctx, nil, "log", state, "--limit", strconv.Itoa(limit))
	}
	return call[LogResult](c, ctx, nil, "log", state)
}
func (c *Client) Diff(ctx context.Context, state string, v1, v2 int) (DiffResult, error) {
	return call[DiffResult](c, ctx, nil, "diff", state, strconv.Itoa(v1), strconv.Itoa(v2))
}
func (c *Client) Rollback(ctx context.Context, state string, version int) (RollbackResult, error) {
	return call[RollbackResult](c, ctx, nil, "rollback", state, strconv.Itoa(version))
}

// EnvSet sends values over stdin (`env import <state> -`), never argv.
func (c *Client) EnvSet(ctx context.Context, state string, vars map[string]string) (EnvResult, error) {
	keys := make([]string, 0, len(vars))
	for k := range vars {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		if strings.ContainsAny(k, "=\n") || strings.Contains(vars[k], "\n") {
			return EnvResult{}, newError("usage", "invalid env entry "+k, 2)
		}
		b.WriteString(k + "=" + vars[k] + "\n")
	}
	return call[EnvResult](c, ctx, []byte(b.String()), "env", "import", state, "-")
}
func (c *Client) EnvUnset(ctx context.Context, state string, keys ...string) (EnvResult, error) {
	return call[EnvResult](c, ctx, nil, append([]string{"env", "unset", state}, keys...)...)
}

// EnvGet returns all vars, or just key when non-empty.
func (c *Client) EnvGet(ctx context.Context, state, key string) (map[string]string, error) {
	args := []string{"env", "get", state}
	if key != "" {
		args = append(args, key)
	}
	r, err := call[struct {
		Env map[string]string `json:"env"`
	}](c, ctx, nil, args...)
	return r.Env, err
}

type ToolAddOptions struct {
	Channel  string
	Prefetch bool
	Required bool
}

func (c *Client) ToolAdd(ctx context.Context, state, tool string, o ToolAddOptions) (ToolResult, error) {
	args := []string{"tool", "add", state, tool}
	if o.Channel != "" {
		args = append(args, "--channel", o.Channel)
	}
	if o.Prefetch {
		args = append(args, "--prefetch")
	}
	if o.Required {
		args = append(args, "--required")
	}
	return call[ToolResult](c, ctx, nil, args...)
}
func (c *Client) Status(ctx context.Context) (StatusResult, error) {
	return call[StatusResult](c, ctx, nil, "status")
}
func (c *Client) States(ctx context.Context) ([]StateInfo, error) {
	r, err := call[struct {
		States []StateInfo `json:"states"`
	}](c, ctx, nil, "states")
	return r.States, err
}
