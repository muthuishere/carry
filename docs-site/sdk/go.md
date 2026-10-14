# Go

Module `github.com/muthuishere/carry/sdk/go` in [`sdk/go`](https://github.com/muthuishere/carry/tree/main/sdk/go). Standard library only.

## Install

```sh
go get github.com/muthuishere/carry/sdk/go@main
```

## Example

```go
package main

import (
	"context"
	"errors"
	"fmt"
	"log"

	carry "github.com/muthuishere/carry/sdk/go"
)

func main() {
	ctx := context.Background()
	c := carry.New(carry.Options{Profile: "dev"})
	state := "web-1"

	if _, err := c.EnvSet(ctx, state, map[string]string{"LOG_LEVEL": "info"}); err != nil {
		log.Fatal(err)
	}
	if _, err := c.Put(ctx, state+":/etc/app.yaml", []byte("listen: 8080\n"), 0o644); err != nil {
		log.Fatal(err)
	}
	data, err := c.Get(ctx, state+":/etc/app.yaml")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Print(string(data))

	if _, err := c.Rollback(ctx, state, 3); errors.Is(err, carry.ErrConflict) {
		// someone else is rolling back; retry later
	}
}
```

## API

```go
func New(opts Options) *Client
(c *Client) Run(ctx, args []string, stdin []byte) (json.RawMessage, error)
Put(ctx, path string, data []byte, mode int) (PutResult, error)   // mode 0 = default
Get(ctx, path) ([]byte, error)
Ls / Rm / Mv / Log(ctx, state, limit int) / Diff / Rollback
EnvSet(ctx, state, map[string]string) / EnvUnset(ctx, state, keys...) / EnvGet(ctx, state, key)
ToolAdd(ctx, state, tool string, ToolAddOptions) / Status / States
```

Errors are `*carry.Error` (`Code`, `Message`, `ExitCode`); match kinds with `errors.Is(err, carry.ErrUsage | ErrUnreachable | ErrRefused | ErrConflict)`; unknown failures use `ErrCarry`.

Tests: `cd sdk/go && go test ./...`. Example: `go run ./examples/basic`.
