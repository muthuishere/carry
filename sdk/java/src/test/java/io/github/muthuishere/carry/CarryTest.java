package io.github.muthuishere.carry;

import static org.junit.jupiter.api.Assertions.*;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class CarryTest {
    @TempDir Path tmp;
    Path db;

    @BeforeEach
    void setUp() { db = tmp.resolve("db.json"); }

    Carry.Builder client() {
        String bin = System.getenv("CARRY_BIN");
        if (bin == null) bin = Path.of("../testdata/fake-carry").toAbsolutePath().toString();
        return Carry.builder().bin(bin).env("FAKE_CARRY_DB", db.toString());
    }

    String lastArgv() throws Exception {
        Map<?, ?> d = (Map<?, ?>) Json.parse(Files.readString(db));
        return ((Map<?, ?>) d.get("last_call")).get("argv").toString();
    }

    @Test
    void putGetRoundTrip() throws Exception {
        Carry c = client().profile("dev").build();
        c.put("web-1:/etc/app.yaml", "x: 1");
        assertEquals("x: 1", new String(c.get("web-1:/etc/app.yaml"), StandardCharsets.UTF_8));
        assertTrue(lastArgv().contains("--profile"));
    }

    @Test
    void binaryAndMode() {
        Carry c = client().build();
        c.put("s:/bin", new byte[] {0, (byte) 255, 1}, 0600);
        assertArrayEquals(new byte[] {0, (byte) 255, 1}, c.get("s:/bin"));
        Map<?, ?> e = (Map<?, ?>) ((List<?>) c.ls("s").get("entries")).get(0);
        assertEquals(0600L, e.get("mode"));
    }

    @Test
    void fileOps() {
        Carry c = client().build();
        c.put("s:/a", "1");
        c.mv("s:/a", "/b");
        assertEquals("/b", ((Map<?, ?>) ((List<?>) c.ls("s").get("entries")).get(0)).get("path"));
        c.rm("s:/b");
        assertEquals(2, ((List<?>) c.log("s", 2).get("versions")).size());
        assertEquals(1L, c.rollback("s", 1).get("restored_from"));
        assertEquals(List.of(), c.diff("s", 1, 2).get("added"));
        assertEquals(List.of(Map.of("name", "s", "version", 4L)), c.states());
    }

    @Test
    void envValuesNeverInArgv() throws Exception {
        Carry c = client().build();
        c.envSet("s", Map.of("DB_PASSWORD", "hunter2", "MODE", "prod"));
        assertFalse(lastArgv().contains("hunter2"));
        assertEquals(Map.of("MODE", "prod"), c.envGet("s", "MODE"));
        c.envUnset("s", "MODE");
        assertEquals(Map.of("DB_PASSWORD", "hunter2"), c.envGet("s"));
    }

    @Test
    void toolAddAndStatus() {
        Carry c = client().env("CARRY_STATE", "s").build();
        c.toolAdd("s", "jq@1.7", "stable", false, false);
        Map<String, Object> st = c.status();
        assertEquals("s", st.get("state"));
        assertEquals(List.of(Map.of("name", "jq", "status", "ready")), st.get("tools"));
    }

    @Test
    void typedErrors() {
        Carry c = client().build();
        CarryException.Conflict conflict = assertThrows(CarryException.Conflict.class, () -> c.put("conflict:/a", "x"));
        assertEquals(5, conflict.exitCode());
        assertEquals("conflict", conflict.code());
        assertThrows(CarryException.Unreachable.class, () -> c.ls("unreachable"));
        assertEquals("bad_key", assertThrows(CarryException.Refused.class, () -> c.ls("badkey")).code());
        assertEquals("not_found", assertThrows(CarryException.Usage.class, () -> c.get("s:/missing")).code());
        assertThrows(CarryException.class, () -> Carry.builder().bin("/nonexistent/carry").build().states());
    }

    @Test
    void genericRun() {
        assertEquals(Map.of("states", List.of()), client().build().run(List.of("states")));
    }
}
