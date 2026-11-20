import java.util.Arrays;

public final class SessionPipeProbe {
    public static void main(String[] args) throws Exception {
        String placeholder = "fixture-game-token";
        String[] expected = {
            "--accessToken", placeholder,
            "--session", "token:" + placeholder + ":fixture",
            "--gameDir", "C:\\验收 游戏\\实例",
            "--quickPlaySingleplayer", "雪地 \"试验\"",
            "--empty", ""
        };
        boolean exact = args.length == expected.length + 2
            && Arrays.equals(expected, Arrays.copyOf(args, expected.length))
            && args[expected.length].equals("--long-input")
            && args[expected.length + 1].length() == 128 * 1024
            && args[expected.length + 1].chars().allMatch(c -> c == 'x');
        System.out.println(exact ? "PIPE_ARGS_OK" : "PIPE_ARGS_INVALID");
        System.out.println(System.getProperty("sun.java.command", "").contains(placeholder)
            ? "JAVA_COMMAND_EXPOSED" : "JAVA_COMMAND_CLEAN");
        System.out.println(System.getProperty("java.class.path", "")
            .equals(System.getenv("LUMI_TEST_MAIN_CLASSES")) ? "GAME_CLASSPATH_OK" : "GAME_CLASSPATH_CHANGED");
        System.out.println("token=" + placeholder);
        Thread.sleep(3500);
    }
}
