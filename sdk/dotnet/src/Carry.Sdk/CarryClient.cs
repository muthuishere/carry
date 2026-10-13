using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Carry.Sdk;

/// <summary>Base error for any failed carry invocation.</summary>
public class CarryException : Exception
{
    public string Code { get; }
    public int ExitCode { get; }
    public CarryException(string code, string message, int exitCode) : base($"{code}: {message}")
    {
        Code = code;
        ExitCode = exitCode;
    }
}
public class UsageException(string c, string m, int e) : CarryException(c, m, e);
public class UnreachableException(string c, string m, int e) : CarryException(c, m, e);
public class RefusedException(string c, string m, int e) : CarryException(c, m, e);
public class ConflictException(string c, string m, int e) : CarryException(c, m, e);

public sealed class CarryOptions
{
    /// <summary>Binary path; defaults to $CARRY_BIN, then "carry" on PATH.</summary>
    public string? Bin { get; init; }
    public string? Profile { get; init; }
    public string? Repo { get; init; }
    public string? Endpoint { get; init; }
    /// <summary>Seconds, passed as --timeout.</summary>
    public int? Timeout { get; init; }
    /// <summary>Extra environment for the child process (put secrets here, never in args).</summary>
    public IDictionary<string, string> Env { get; init; } = new Dictionary<string, string>();
}

/// <summary>Thin wrapper over the carry CLI JSON interface (docs/cli-contract.md).</summary>
public sealed class CarryClient
{
    private readonly CarryOptions _opts;
    public string Bin { get; }

    public CarryClient(CarryOptions? opts = null)
    {
        _opts = opts ?? new CarryOptions();
        Bin = _opts.Bin ?? Environment.GetEnvironmentVariable("CARRY_BIN") ?? "carry";
    }

    internal static CarryException ToException(string code, string message, int exit) => code switch
    {
        "usage" or "readonly" or "not_found" => new UsageException(code, message, exit),
        "unreachable" => new UnreachableException(code, message, exit),
        "refused" or "invalid" or "bad_key" => new RefusedException(code, message, exit),
        "conflict" => new ConflictException(code, message, exit),
        _ => exit switch
        {
            2 => new UsageException(code, message, exit),
            3 => new UnreachableException(code, message, exit),
            4 => new RefusedException(code, message, exit),
            5 => new ConflictException(code, message, exit),
            _ => new CarryException(code, message, exit),
        },
    };

    /// <summary>Runs any carry command with --json and returns the envelope's result.</summary>
    public async Task<JsonNode?> RunAsync(IEnumerable<string> args, byte[]? stdin = null, CancellationToken ct = default)
    {
        var psi = new ProcessStartInfo(Bin)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add("--json");
        if (_opts.Profile != null) { psi.ArgumentList.Add("--profile"); psi.ArgumentList.Add(_opts.Profile); }
        if (_opts.Repo != null) { psi.ArgumentList.Add("--repo"); psi.ArgumentList.Add(_opts.Repo); }
        if (_opts.Endpoint != null) { psi.ArgumentList.Add("--endpoint"); psi.ArgumentList.Add(_opts.Endpoint); }
        if (_opts.Timeout != null) { psi.ArgumentList.Add("--timeout"); psi.ArgumentList.Add(_opts.Timeout.ToString()!); }
        foreach (var a in args) psi.ArgumentList.Add(a);
        foreach (var (k, v) in _opts.Env) psi.Environment[k] = v;

        Process p;
        try { p = Process.Start(psi)!; }
        catch (Exception e) { throw new CarryException("internal", $"cannot run {Bin}: {e.Message}", 1); }
        using (p)
        {
            var outTask = p.StandardOutput.ReadToEndAsync(ct);
            var errTask = p.StandardError.ReadToEndAsync(ct);
            if (stdin != null) await p.StandardInput.BaseStream.WriteAsync(stdin, ct);
            p.StandardInput.Close();
            var stdout = await outTask;
            var stderr = await errTask;
            await p.WaitForExitAsync(ct);

            JsonNode? env;
            try { env = JsonNode.Parse(stdout); }
            catch (JsonException) { throw new CarryException("internal", $"unparseable output: {stderr.Trim()}", p.ExitCode); }
            if (env?["ok"]?.GetValue<bool>() == true) return env["result"];
            var err = env?["error"];
            throw ToException(err?["code"]?.GetValue<string>() ?? "internal",
                err?["message"]?.GetValue<string>() ?? "unknown error", p.ExitCode);
        }
    }

    private async Task<T> Call<T>(byte[]? stdin, params string[] args)
    {
        var node = await RunAsync(args, stdin);
        return node.Deserialize<T>(JsonOpts)!;
    }

    private static readonly JsonSerializerOptions JsonOpts = new() { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };

    public Task<PutResult> PutAsync(string path, byte[] data, int? mode = null)
    {
        var args = new List<string> { "put", path, "-" };
        if (mode != null) { args.Add("--mode"); args.Add(Convert.ToString(mode.Value, 8)); }
        return Call<PutResult>(data, args.ToArray());
    }
    public Task<PutResult> PutAsync(string path, string data, int? mode = null) => PutAsync(path, Encoding.UTF8.GetBytes(data), mode);

    public async Task<byte[]> GetAsync(string path)
    {
        var r = await RunAsync(["get", path]);
        return Convert.FromBase64String(r!["content_base64"]!.GetValue<string>());
    }

    public Task<LsResult> LsAsync(string target) => Call<LsResult>(null, "ls", target);
    public Task<RmResult> RmAsync(string path) => Call<RmResult>(null, "rm", path);
    public Task<MvResult> MvAsync(string from, string to) => Call<MvResult>(null, "mv", from, to);
    public Task<LogResult> LogAsync(string state, int? limit = null) =>
        limit is int l ? Call<LogResult>(null, "log", state, "--limit", l.ToString()) : Call<LogResult>(null, "log", state);
    public Task<DiffResult> DiffAsync(string state, int v1, int v2) => Call<DiffResult>(null, "diff", state, v1.ToString(), v2.ToString());
    public Task<RollbackResult> RollbackAsync(string state, int version) => Call<RollbackResult>(null, "rollback", state, version.ToString());

    /// <summary>Values travel over stdin (env import &lt;state&gt; -), never argv.</summary>
    public Task<EnvResult> EnvSetAsync(string state, IDictionary<string, string> vars)
    {
        var sb = new StringBuilder();
        foreach (var (k, v) in vars)
        {
            if (k.Contains('=') || k.Contains('\n') || v.Contains('\n'))
                throw new UsageException("usage", $"invalid env entry {k}", 2);
            sb.Append(k).Append('=').Append(v).Append('\n');
        }
        return Call<EnvResult>(Encoding.UTF8.GetBytes(sb.ToString()), "env", "import", state, "-");
    }
    public Task<EnvResult> EnvUnsetAsync(string state, params string[] keys) =>
        Call<EnvResult>(null, ["env", "unset", state, .. keys]);
    public async Task<Dictionary<string, string>> EnvGetAsync(string state, string? key = null)
    {
        var r = key == null ? await RunAsync(["env", "get", state]) : await RunAsync(["env", "get", state, key]);
        return r!["env"].Deserialize<Dictionary<string, string>>()!;
    }
    public Task<ToolResult> ToolAddAsync(string state, string tool, string? channel = null, bool prefetch = false, bool required = false)
    {
        var a = new List<string> { "tool", "add", state, tool };
        if (channel != null) { a.Add("--channel"); a.Add(channel); }
        if (prefetch) a.Add("--prefetch");
        if (required) a.Add("--required");
        return Call<ToolResult>(null, a.ToArray());
    }
    public Task<StatusResult> StatusAsync() => Call<StatusResult>(null, "status");
    public async Task<List<StateInfo>> StatesAsync() => (await Call<StatesResult>(null, "states")).States;
}

public record PutResult(string State, string Path, int Version, long Size, bool Uploaded);
public record LsEntry(string Path, string Kind, long Size, int Mode);
public record LsResult(string State, int Version, List<LsEntry> Entries);
public record RmResult(string State, string Path, int Version);
public record MvResult(string State, string From, string To, int Version);
public record LogEntry(int Version, string Op, List<string> Args, string At, string By);
public record LogResult(string State, List<LogEntry> Versions);
public record DiffResult(List<string> Added, List<string> Removed, List<string> Changed, List<string> EnvChanged, List<string> AppsChanged);
public record RollbackResult(string State, int Version, int RestoredFrom);
public record EnvResult(string State, int Version, List<string> Keys);
public record ToolResult(string State, int Version, string Tool);
public record ToolStatus(string Name, string Status);
public record StatusResult(string? State, string Mode, int? Version, string Root, int? LastRestoreMs, string? Error, List<ToolStatus> Tools);
public record StateInfo(string Name, int Version);
public record StatesResult(List<StateInfo> States);
