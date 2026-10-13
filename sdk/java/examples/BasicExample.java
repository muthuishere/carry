// gradle runExample   (uses $CARRY_BIN or `carry` on PATH)
import io.github.muthuishere.carry.Carry;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

public class BasicExample {
    public static void main(String[] args) {
        Carry.Builder b = Carry.builder();
        if (System.getenv("CARRY_PROFILE") != null) b.profile(System.getenv("CARRY_PROFILE"));
        Carry carry = b.build();
        String state = "web-1";

        carry.envSet(state, Map.of("LOG_LEVEL", "info", "PORT", "8080"));
        carry.put(state + ":/etc/app.yaml", "listen: 8080\n".getBytes(StandardCharsets.UTF_8), 0644);
        System.out.println(new String(carry.get(state + ":/etc/app.yaml"), StandardCharsets.UTF_8));
        carry.toolAdd(state, "jq@1.7");
        for (Object o : (List<?>) carry.log(state).get("versions")) {
            Map<?, ?> v = (Map<?, ?>) o;
            System.out.println(v.get("version") + " " + v.get("op") + " " + String.join(" ", (List<String>) v.get("args")));
        }
    }
}
