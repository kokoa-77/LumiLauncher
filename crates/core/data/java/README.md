# Lumi Java 启动入口

`SessionPipe.java` 读取 Rust 父进程通过标准输入管道交付的游戏入口和参数，然后调用原有 Minecraft 或加载器入口。JVM 参数仍由 Java 启动器解析。这个类在引导游戏期间不联网，不进行认证，不写游戏参数文件。它可在 Java 8 及之后的运行时运行。

`SessionPipe.class` 由本目录源码生成并由 Rust 内嵌。为保持 Windows 构建可在无 JDK 的环境复现，日常 Cargo 构建直接使用这份字节码。修改源码后，从项目根目录用 JDK 17 或更新版重新生成，将 `java-build/com/lumilauncher/bootstrap/SessionPipe.class` 复制回本目录，并更新下面的校验和。

```powershell
javac -encoding UTF-8 --release 8 -Xlint:-options -g:none -d java-build crates/core/data/java/SessionPipe.java
Get-FileHash -Algorithm SHA256 java-build/com/lumilauncher/bootstrap/SessionPipe.class
```

当前文件 SHA-256 为 `D68F9F0EE5A83EA1C529BBC025DFDF8286E8C11CB80F04149144A13123F0A85D`，由 JDK 25.0.1 生成。

Windows 回归使用 `SessionPipeProbe.java`，它只接收固定模拟参数。使用 `javac -encoding UTF-8 --release 8 -Xlint:-options -d java-probe crates/core/data/java/SessionPipeProbe.java` 编译，将 `LUMI_TEST_MAIN_CLASSES` 设为 `java-probe` 的绝对路径，`LUMI_TEST_JAVA` 设为待验证的 `javaw.exe`，`LUMI_TEST_PWSH` 设为 `pwsh.exe` 的绝对路径。运行对应测试时，当前账户需要能读取自己的 Windows 进程命令行。

```powershell
cargo test -p launcher-core --lib passes_game_arguments_without_exposing_the_session_to_process_audit -- --ignored
```

该测试传递 128 KiB 的普通参数并开启详细 JVM 类加载日志，覆盖父进程先读输出、再向子进程写入的路径。同时检查原有类路径和含空格、引号及中文的游戏参数。

额外的 Forge 模块复测使用名为 `cpw.mods.bootstraplauncher` 的 Java 模块。把测试入口类放到 `cpw.mods.bootstraplauncher` 包并命名为 `BootstrapLauncher`，令 `module-info.java` 仅声明 `module cpw.mods.bootstraplauncher {}`。编译为模块 JAR，保存到以 `cpw/mods/bootstraplauncher/1.0.0/bootstraplauncher-1.0.0.jar` 结尾的位置。将 `LUMI_TEST_FORGE_MODULE` 指向这个 JAR，使用 Java 17 重跑同一测试。这样复制旧 Forge 的入口可见性条件，运行的始终是模拟程序。
