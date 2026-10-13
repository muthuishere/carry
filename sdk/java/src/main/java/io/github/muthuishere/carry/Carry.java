package io.github.muthuishere.carry;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;

/**
 * Thin wrapper over the carry CLI JSON interface (docs/cli-contract.md).
 * Results are the envelope's {@code result} as Map/List/String/Long/Boolean.
 */
public final class Carry {
    private final String bin;
    private final String profile, repo, endpoint;
    private final Integer timeout;
    private final Map<String, String> env;

    private Carry(Builder b) {
        String fromEnv = System.getenv("CARRY_BIN");
        this.bin = b.bin != null ? b.bin : (fromEnv != null && !fromEnv.isEmpty() ? fromEnv : "carry");
        this.profile = b.profile;
        this.repo = b.repo;
        this.endpoint = b.endpoint;
        this.timeout = b.timeout;
        this.env = Map.copyOf(b.env);
    }

    public static Builder builder() { return new Builder(); }

    public static Carry create() { return builder().build(); }

    public static final class Builder {
        private String bin, profile, repo, endpoint;
        private Integer timeout;
        private final Map<String, String> env = new HashMap<>();

        public Builder bin(String v) { bin = v; return this; }
        public Builder profile(String v) { profile = v; return this; }
        public Builder repo(String v) { repo = v; return this; }
        public Builder endpoint(String v) { endpoint = v; return this; }
        /** Seconds, passed as --timeout. */
        public Builder timeout(int v) { timeout = v; return this; }
        /** Extra env for the child; put secrets here, never in args. */
        public Builder env(String k, String v) { env.put(k, v); return this; }
        public Builder env(Map<String, String> m) { env.putAll(m); return this; }
        public Carry build() { return new Carry(this); }
    }

    /** Runs any carry command with --json and returns the parsed result. */
    public Object run(List<String> args) { return run(args, null); }

    public Object run(List<String> args, byte[] stdin) {
        List<String> cmd = new ArrayList<>();
        cmd.add(bin);
        cmd.add("--json");
        if (profile != null) { cmd.add("--profile"); cmd.add(profile); }
        if (repo != null) { cmd.add("--repo"); cmd.add(repo); }
        if (endpoint != null) { cmd.add("--endpoint"); cmd.add(endpoint); }
        if (timeout != null) { cmd.add("--timeout"); cmd.add(timeout.toString()); }
        cmd.addAll(args);
        ProcessBuilder pb = new ProcessBuilder(cmd);
        pb.environment().putAll(env);
        Process p;
        try {
            p = pb.start();
        } catch (IOException e) {
            throw new CarryException("internal", "cannot run " + bin + ": " + e.getMessage(), 1);
        }
        try {
            CompletableFuture<byte[]> out = drain(p.getInputStream());
            CompletableFuture<byte[]> err = drain(p.getErrorStream());
            try (OutputStream in = p.getOutputStream()) {
                if (stdin != null) in.write(stdin);
            }
            int exit = p.waitFor();
            String stdout = new String(out.join(), StandardCharsets.UTF_8);
            Object parsed;
            try {
                parsed = Json.parse(stdout);
            } catch (RuntimeException e) {
                throw new CarryException("internal",
                        "unparseable output: " + new String(err.join(), StandardCharsets.UTF_8).trim(), exit);
            }
            Map<?, ?> envelope = (Map<?, ?>) parsed;
            if (Boolean.TRUE.equals(envelope.get("ok"))) return envelope.get("result");
            Map<?, ?> e = envelope.get("error") instanceof Map ? (Map<?, ?>) envelope.get("error") : Map.of();
            Object code = e.get("code"), msg = e.get("message");
            throw CarryException.of(code == null ? "internal" : code.toString(),
                    msg == null ? "unknown error" : msg.toString(), exit);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            p.destroyForcibly();
            throw new CarryException("internal", "interrupted", 1);
        }
    }

    private static CompletableFuture<byte[]> drain(InputStream s) {
        return CompletableFuture.supplyAsync(() -> {
            try (s; ByteArrayOutputStream b = new ByteArrayOutputStream()) {
                s.transferTo(b);
                return b.toByteArray();
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        });
    }

    @SuppressWarnings("unchecked")
    private Map<String, Object> obj(byte[] stdin, String... args) {
        return (Map<String, Object>) run(Arrays.asList(args), stdin);
    }

    public Map<String, Object> put(String path, byte[] data) { return put(path, data, null); }

    public Map<String, Object> put(String path, String data) { return put(path, data.getBytes(StandardCharsets.UTF_8), null); }

    /** @param mode file mode such as 0644, or null for the default */
    public Map<String, Object> put(String path, byte[] data, Integer mode) {
        if (mode == null) return obj(data, "put", path, "-");
        return obj(data, "put", path, "-", "--mode", Integer.toOctalString(mode));
    }

    public byte[] get(String path) {
        return java.util.Base64.getDecoder().decode((String) obj(null, "get", path).get("content_base64"));
    }

    public Map<String, Object> ls(String target) { return obj(null, "ls", target); }
    public Map<String, Object> rm(String path) { return obj(null, "rm", path); }
    public Map<String, Object> mv(String from, String to) { return obj(null, "mv", from, to); }
    public Map<String, Object> log(String state) { return obj(null, "log", state); }
    public Map<String, Object> log(String state, int limit) { return obj(null, "log", state, "--limit", String.valueOf(limit)); }
    public Map<String, Object> diff(String state, int v1, int v2) {
        return obj(null, "diff", state, String.valueOf(v1), String.valueOf(v2));
    }
    public Map<String, Object> rollback(String state, int version) { return obj(null, "rollback", state, String.valueOf(version)); }

    /** Values travel over stdin ({@code env import <state> -}), never argv. */
    public Map<String, Object> envSet(String state, Map<String, String> vars) {
        StringBuilder b = new StringBuilder();
        for (Map.Entry<String, String> e : vars.entrySet()) {
            if (e.getKey().contains("=") || e.getKey().contains("\n") || e.getValue().contains("\n"))
                throw new CarryException.Usage("usage", "invalid env entry " + e.getKey(), 2);
            b.append(e.getKey()).append('=').append(e.getValue()).append('\n');
        }
        return obj(b.toString().getBytes(StandardCharsets.UTF_8), "env", "import", state, "-");
    }

    public Map<String, Object> envUnset(String state, String... keys) {
        List<String> a = new ArrayList<>(List.of("env", "unset", state));
        a.addAll(Arrays.asList(keys));
        return obj(null, a.toArray(new String[0]));
    }

    public Map<String, String> envGet(String state) { return envGet(state, null); }

    @SuppressWarnings("unchecked")
    public Map<String, String> envGet(String state, String key) {
        Map<String, Object> r = key == null ? obj(null, "env", "get", state) : obj(null, "env", "get", state, key);
        return new LinkedHashMap<>((Map<String, String>) r.get("env"));
    }

    public Map<String, Object> toolAdd(String state, String tool) { return toolAdd(state, tool, null, false, false); }

    public Map<String, Object> toolAdd(String state, String tool, String channel, boolean prefetch, boolean required) {
        List<String> a = new ArrayList<>(List.of("tool", "add", state, tool));
        if (channel != null) { a.add("--channel"); a.add(channel); }
        if (prefetch) a.add("--prefetch");
        if (required) a.add("--required");
        return obj(null, a.toArray(new String[0]));
    }

    public Map<String, Object> status() { return obj(null, "status"); }

    @SuppressWarnings("unchecked")
    public List<Map<String, Object>> states() { return (List<Map<String, Object>>) obj(null, "states").get("states"); }
}
