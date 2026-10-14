# Java

Package `io.github.muthuishere.carry` in [`sdk/java`](https://github.com/muthuishere/carry/tree/main/sdk/java). Java 17+, no runtime dependencies (Gradle build).

## Install

```sh
git clone https://github.com/muthuishere/carry
cd carry/sdk/java && gradle jar        # then add build/libs/*.jar to your classpath
```

Or include it as a Gradle composite build (`includeBuild("path/to/carry/sdk/java")`).

## Example

```java
import io.github.muthuishere.carry.Carry;
import io.github.muthuishere.carry.CarryException;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

Carry carry = Carry.builder().profile("dev").build();
String state = "web-1";

carry.envSet(state, Map.of("LOG_LEVEL", "info", "PORT", "8080"));
carry.put(state + ":/etc/app.yaml", "listen: 8080\n".getBytes(StandardCharsets.UTF_8), 0644);
System.out.println(new String(carry.get(state + ":/etc/app.yaml"), StandardCharsets.UTF_8));
carry.toolAdd(state, "jq@1.7");
for (Object o : (List<?>) carry.log(state).get("versions")) {
    Map<?, ?> v = (Map<?, ?>) o;
    System.out.println(v.get("version") + " " + v.get("op"));
}

try { carry.rollback(state, 3); }
catch (CarryException.Conflict e) { /* retry later */ }
```

## API

Builder: `bin`, `profile`, `repo`, `endpoint`, `timeout`, `env(k, v)` / `env(map)`.
Methods: `run(List<String>[, byte[] stdin])`, `put(path, byte[] | String[, Integer mode])`, `get`, `ls`, `rm`, `mv`, `log(state[, limit])`, `diff`, `rollback`, `envSet`, `envUnset(state, String...)`, `envGet(state[, key])`, `toolAdd(state, tool[, channel, prefetch, required])`, `status`, `states`. Results are `Map<String, Object>`.

Exceptions: `CarryException` with nested `Usage`, `Unreachable`, `Refused`, `Conflict`.

Tests: `cd sdk/java && gradle test`. Example: `gradle runExample`.
