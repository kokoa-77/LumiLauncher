package com.lumilauncher.bootstrap;

import java.io.BufferedInputStream;
import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.File;
import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/** Delivers the prepared Minecraft main arguments through the inherited input pipe. */
public final class SessionPipe {
    private static final int MAX_ARGUMENTS = 4096;
    private static final int MAX_BYTES = 1024 * 1024;

    private SessionPipe() {}

    public static void main(String[] commandLine) throws Throwable {
        String mainClass;
        String[] args;
        try (DataInputStream input = new DataInputStream(new BufferedInputStream(System.in))) {
            int count = input.readInt();
            if (commandLine.length != 0 || count < 0 || count > MAX_ARGUMENTS) {
                throw new IOException();
            }
            int[] remaining = {MAX_BYTES - Integer.BYTES};
            mainClass = readString(input, remaining);
            args = new String[count];
            for (int i = 0; i < args.length; i++) {
                args[i] = readString(input, remaining);
            }
            if (input.read() != -1) {
                throw new IOException();
            }
        } catch (IOException error) {
            throw new IOException("Lumi could not read the prepared game arguments.");
        }

        System.setIn(new ByteArrayInputStream(new byte[0]));
        // BootstrapLauncher reads this property to reconstruct the game's module path.
        String classPath = System.getProperty("java.class.path", "");
        int separator = classPath.indexOf(File.pathSeparatorChar);
        if (separator >= 0) {
            System.setProperty("java.class.path", classPath.substring(separator + 1));
        }
        Class<?> entry = Class.forName(mainClass, false, ClassLoader.getSystemClassLoader());
        try {
            entry.getMethod("main", String[].class).invoke(null, (Object) args);
        } catch (InvocationTargetException error) {
            throw error.getCause();
        }
    }

    private static String readString(DataInputStream input, int[] remaining) throws IOException {
        int length = input.readInt();
        remaining[0] -= Integer.BYTES;
        if (length < 0 || length > remaining[0]) {
            throw new IOException();
        }
        remaining[0] -= length;
        byte[] bytes = new byte[length];
        try {
            input.readFully(bytes);
            return new String(bytes, StandardCharsets.UTF_8);
        } finally {
            Arrays.fill(bytes, (byte) 0);
        }
    }
}
