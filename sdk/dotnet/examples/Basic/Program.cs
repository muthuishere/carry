// dotnet run --project examples/Basic   (uses $CARRY_BIN or `carry` on PATH)
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
