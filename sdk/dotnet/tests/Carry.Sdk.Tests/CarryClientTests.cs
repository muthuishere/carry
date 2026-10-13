using System.Text;
using System.Text.Json.Nodes;
using Carry.Sdk;
using Xunit;

namespace Carry.Sdk.Tests;

public class CarryClientTests : IDisposable
{
    private readonly string _dir = Directory.CreateTempSubdirectory("carry-dotnet-").FullName;
    private readonly string _db;
    private readonly string _bin;

    public CarryClientTests()
    {
        _db = Path.Combine(_dir, "db.json");
        _bin = Environment.GetEnvironmentVariable("CARRY_BIN")
            ?? Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../../../testdata/fake-carry"));
    }

    public void Dispose() => Directory.Delete(_dir, true);

    private CarryClient Client(string? profile = null, Dictionary<string, string>? env = null)
    {
        var e = env ?? new Dictionary<string, string>();
        e["FAKE_CARRY_DB"] = _db;
        return new CarryClient(new CarryOptions { Bin = _bin, Profile = profile, Env = e });
    }

    private string LastArgv() => JsonNode.Parse(File.ReadAllText(_db))!["last_call"]!["argv"]!.ToJsonString();

    [Fact]
    public async Task PutGetRoundTrip()
    {
        var c = Client("dev");
        await c.PutAsync("web-1:/etc/app.yaml", "x: 1");
        Assert.Equal("x: 1", Encoding.UTF8.GetString(await c.GetAsync("web-1:/etc/app.yaml")));
        Assert.Contains("--profile", LastArgv());
    }

    [Fact]
    public async Task BinaryAndMode()
    {
        var c = Client();
        await c.PutAsync("s:/bin", new byte[] { 0, 255, 1 }, Convert.ToInt32("600", 8));
        Assert.Equal(new byte[] { 0, 255, 1 }, await c.GetAsync("s:/bin"));
        Assert.Equal(Convert.ToInt32("600", 8), (await c.LsAsync("s")).Entries[0].Mode);
    }

    [Fact]
    public async Task FileOps()
    {
        var c = Client();
        await c.PutAsync("s:/a", "1");
        await c.MvAsync("s:/a", "/b");
        Assert.Equal("/b", Assert.Single((await c.LsAsync("s")).Entries).Path);
        await c.RmAsync("s:/b");
        Assert.Equal(2, (await c.LogAsync("s", 2)).Versions.Count);
        Assert.Equal(1, (await c.RollbackAsync("s", 1)).RestoredFrom);
        Assert.Empty((await c.DiffAsync("s", 1, 2)).Added);
        Assert.Equal(new StateInfo("s", 4), Assert.Single(await c.StatesAsync()));
    }

    [Fact]
    public async Task EnvValuesNeverInArgv()
    {
        var c = Client();
        await c.EnvSetAsync("s", new Dictionary<string, string> { ["DB_PASSWORD"] = "hunter2", ["MODE"] = "prod" });
        Assert.DoesNotContain("hunter2", LastArgv());
        Assert.Equal("prod", (await c.EnvGetAsync("s", "MODE"))["MODE"]);
        await c.EnvUnsetAsync("s", "MODE");
        Assert.Equal(new Dictionary<string, string> { ["DB_PASSWORD"] = "hunter2" }, await c.EnvGetAsync("s"));
    }

    [Fact]
    public async Task ToolAddAndStatus()
    {
        var c = Client(env: new Dictionary<string, string> { ["CARRY_STATE"] = "s" });
        await c.ToolAddAsync("s", "jq@1.7", channel: "stable");
        var st = await c.StatusAsync();
        Assert.Equal("s", st.State);
        Assert.Equal("jq", Assert.Single(st.Tools).Name);
    }

    [Fact]
    public async Task TypedErrors()
    {
        var c = Client();
        var conflict = await Assert.ThrowsAsync<ConflictException>(() => c.PutAsync("conflict:/a", "x"));
        Assert.Equal(5, conflict.ExitCode);
        Assert.Equal("conflict", conflict.Code);
        await Assert.ThrowsAsync<UnreachableException>(() => c.LsAsync("unreachable"));
        var bad = await Assert.ThrowsAsync<RefusedException>(() => c.LsAsync("badkey"));
        Assert.Equal("bad_key", bad.Code);
        var nf = await Assert.ThrowsAsync<UsageException>(() => c.GetAsync("s:/missing"));
        Assert.Equal("not_found", nf.Code);
        await Assert.ThrowsAnyAsync<CarryException>(() =>
            new CarryClient(new CarryOptions { Bin = "/nonexistent/carry" }).StatesAsync());
    }

    [Fact]
    public async Task GenericRun()
    {
        var r = await Client().RunAsync(["states"]);
        Assert.Empty(r!["states"]!.AsArray());
    }
}
