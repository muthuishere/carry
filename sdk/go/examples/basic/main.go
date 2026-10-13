// go run ./examples/basic   (uses $CARRY_BIN or `carry` on PATH)
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"

	carry "github.com/muthuishere/carry/sdk/go"
)

func main() {
	ctx := context.Background()
	c := carry.New(carry.Options{Profile: os.Getenv("CARRY_PROFILE")})
	state := "web-1"

	must(c.EnvSet(ctx, state, map[string]string{"LOG_LEVEL": "info", "PORT": "8080"}))
	must(c.Put(ctx, state+":/etc/app.yaml", []byte("listen: 8080\n"), 0o644))
	data := must(c.Get(ctx, state+":/etc/app.yaml"))
	fmt.Println(string(data))
	must(c.ToolAdd(ctx, state, "jq@1.7", carry.ToolAddOptions{}))
	for _, v := range must(c.Log(ctx, state, 0)).Versions {
		fmt.Println(v.Version, v.Op, strings.Join(v.Args, " "))
	}
}

func must[T any](v T, err error) T {
	if err != nil {
		log.Fatal(err)
	}
	return v
}
