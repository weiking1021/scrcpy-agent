package io.github.weiking1021.scrcpyagent.android;

interface IWindowControl {
    void destroy() = 16777114; // Reserved by Shizuku for UserService cleanup.
    String launch(String key, String token) = 1;
    String resize(int task, int left, int top, int right, int bottom) = 2;
}
