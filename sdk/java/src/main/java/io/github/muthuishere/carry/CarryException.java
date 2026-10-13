package io.github.muthuishere.carry;

/** Base error for any failed carry invocation; subclasses map the CLI exit codes. */
public class CarryException extends RuntimeException {
    private final String code;
    private final int exitCode;

    public CarryException(String code, String message, int exitCode) {
        super(code + ": " + message);
        this.code = code;
        this.exitCode = exitCode;
    }

    public String code() { return code; }
    public int exitCode() { return exitCode; }

    /** Exit 2: usage, readonly, not_found. */
    public static class Usage extends CarryException {
        public Usage(String c, String m, int e) { super(c, m, e); }
    }
    /** Exit 3: store unreachable. */
    public static class Unreachable extends CarryException {
        public Unreachable(String c, String m, int e) { super(c, m, e); }
    }
    /** Exit 4: refused, invalid, bad_key. */
    public static class Refused extends CarryException {
        public Refused(String c, String m, int e) { super(c, m, e); }
    }
    /** Exit 5: conflict not resolved after retries. */
    public static class Conflict extends CarryException {
        public Conflict(String c, String m, int e) { super(c, m, e); }
    }

    static CarryException of(String code, String message, int exit) {
        switch (code) {
            case "usage": case "readonly": case "not_found": return new Usage(code, message, exit);
            case "unreachable": return new Unreachable(code, message, exit);
            case "refused": case "invalid": case "bad_key": return new Refused(code, message, exit);
            case "conflict": return new Conflict(code, message, exit);
            default:
        }
        switch (exit) {
            case 2: return new Usage(code, message, exit);
            case 3: return new Unreachable(code, message, exit);
            case 4: return new Refused(code, message, exit);
            case 5: return new Conflict(code, message, exit);
            default: return new CarryException(code, message, exit);
        }
    }
}
