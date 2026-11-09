package io.github.weiking1021.scrcpyagent.android

import android.os.Bundle
import android.view.WindowInsets
import android.view.View

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    window.setDecorFitsSystemWindows(false)
    findViewById<View>(android.R.id.content).setOnApplyWindowInsetsListener { view, insets ->
      val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout() or WindowInsets.Type.ime())
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsets.CONSUMED
    }
  }
}
