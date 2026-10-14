# C# / .NET

Package `Carry.Sdk` in [`sdk/dotnet`](https://github.com/muthuishere/carry/tree/main/sdk/dotnet). Async API, `System.Text.Json`.

## Install

```sh
git clone https://github.com/muthuishere/carry
dotnet add reference carry/sdk/dotnet/src/Carry.Sdk/Carry.Sdk.csproj
```

## Example

```csharp
using System.Text;
using Carry.Sdk;

var carry = new CarryClient(new CarryOptions { Profile = Environment.GetEnvironmentVariable("CARRY_PROFILE") });
const string state = "web-1";

await carry.EnvSetAsync(state, new Dictionary<string, string> { ["LOG_LEVEL"] = "info", ["PORT"] = "8080" });
await carry.PutAsync($"{state}:/etc/app.yaml", "listen: 8080\n", Convert.ToInt32("644", 8));
Console.WriteLine(Encoding.UTF8.GetString(await carry.GetAsync($"{state}:/etc/app.yaml")));
await carry.ToolAddAsync(state, "jq@1.7");
foreach (var v in (await carry.LogAsync(state)).Versions)
    Console.WriteLine($"{v.Version} {v.Op} {string.Join(' ', v.Args)}");

try { await carry.RollbackAsync(state, 3); }
catch (ConflictException) { /* retry later */ }
```

## API

`RunAsync`, `PutAsync(path, byte[] | string, int? mode)`, `GetAsync`, `LsAsync`, `RmAsync`, `MvAsync`, `LogAsync(state, int? limit)`, `DiffAsync`, `RollbackAsync`, `EnvSetAsync`, `EnvUnsetAsync(state, params string[] keys)`, `EnvGetAsync(state, string? key)`, `ToolAddAsync(state, tool, channel, prefetch, required)`, `StatusAsync`, `StatesAsync`. Results are records (`PutResult`, `LsResult`, `LogResult`, ...).

Exceptions: `CarryException` (base), `UsageException`, `UnreachableException`, `RefusedException`, `ConflictException`.

Tests: `cd sdk/dotnet && dotnet test`. Example: `dotnet run --project examples/Basic`.
