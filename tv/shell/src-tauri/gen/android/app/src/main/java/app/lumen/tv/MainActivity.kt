package app.lumen.tv

import android.os.Build
import android.os.Bundle
import android.window.OnBackInvokedDispatcher
import android.view.KeyEvent
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.webkit.ScriptHandler
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

/**
 * The TV app's activity. Only what Android forces into Kotlin lives here (the decisions are Rust's,
 * `tv/shell/src-tauri/src`, reached through [Native]):
 * - the client bundle as a document-start script on https://www.youtube.com (kept as a handle so a
 *   verified update swaps it; the swap applies on the next page load or when the app comes back);
 * - the WebView's requests through the client's rules ([Interceptor]);
 * - the remote: Back asks the client first (YTC_BACK), media keys reach the page as key events.
 */
class MainActivity : TauriActivity() {
  override val handleBackNavigation: Boolean = false
  private var web: WebView? = null
  private var script: ScriptHandler? = null
  private var reloadOnStart = false
  private var stoppedAt = 0L
  private lateinit var host: Host

  override fun onCreate(savedInstanceState: Bundle?) {
    // Before Tauri starts the Rust side: where its files live and how it reaches Android.
    host = Host(this)
    Native.init(filesDir.absolutePath, host)
    super.onCreate(savedInstanceState)
    // Back goes to the client first. Android 13+ delivers it as a back invocation, not a key, and the
    // WebView registers its own (history back): an overlay-priority callback comes before it.
    if (Build.VERSION.SDK_INT >= 33) {
      onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_OVERLAY) { back() }
    } else {
      onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
        override fun handleOnBackPressed() = back()
      })
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    web = webView
    webView.settings.userAgentString = Native.userAgent(webView.settings.userAgentString ?: "")
    webView.isFocusable = true
    webView.isFocusableInTouchMode = true
    installScript(webView)
    // Runs after wry has set its own client (same main-thread message), before the first load.
    webView.post { webView.webViewClient = Interceptor(webView.webViewClient) }
    webView.requestFocus()
  }

  private fun installScript(w: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) return
    script?.remove()
    script = WebViewCompat.addDocumentStartJavaScript(w, Native.script(), setOf("https://www.youtube.com"))
  }

  /** A newer verified bundle is in place (Rust → [Host.bundleReady]). */
  fun swapBundle() {
    val w = web ?: return
    installScript(w)
    reloadOnStart = true
  }

  override fun onStart() {
    super.onStart()
    // A swapped bundle goes live when the owner comes back to the app (never mid-video).
    if (reloadOnStart && stoppedAt > 0) {
      reloadOnStart = false
      web?.reload()
    }
  }

  override fun onResume() {
    super.onResume()
    host.resumeInstall()
  }

  override fun onStop() {
    stoppedAt = System.currentTimeMillis()
    // Leaving the app pauses playback (a TV app never plays on in the background).
    web?.evaluateJavascript("document.querySelectorAll('video,audio').forEach(m=>m.pause())", null)
    super.onStop()
  }

  private fun back() {
    val w = web ?: return finish()
    // "h" = the client handled it, "x" = the client asks to leave the app (Back on the rail at Home), "n" = not
    // handled: the WebView goes back, or the app leaves when there is nothing to go back to.
    w.evaluateJavascript("(()=>{try{const h=!!(globalThis.YTC_BACK&&YTC_BACK());return h?'h':globalThis.YTC_EXIT?'x':'n'}catch(e){return 'n'}})()") { r ->
      android.util.Log.i("LumenTV", "back: client answered $r")
      if (r == "\"h\"") return@evaluateJavascript
      if (r != "\"x\"" && w.canGoBack()) w.goBack() else moveTaskToBack(true)
    }
  }

  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    // Back is the client's first (YTC_BACK), before the WebView or any callback sees the key.
    if (event.keyCode == KeyEvent.KEYCODE_BACK && web != null) {
      if (event.action == KeyEvent.ACTION_UP && !event.isCanceled) back()
      return true
    }
    val name = when (event.keyCode) {
      KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_HEADSETHOOK -> "MediaPlayPause"
      KeyEvent.KEYCODE_MEDIA_PLAY -> "MediaPlay"
      KeyEvent.KEYCODE_MEDIA_PAUSE -> "MediaPause"
      KeyEvent.KEYCODE_MEDIA_REWIND -> "MediaRewind"
      KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> "MediaFastForward"
      KeyEvent.KEYCODE_MEDIA_NEXT -> "MediaTrackNext"
      KeyEvent.KEYCODE_MEDIA_PREVIOUS -> "MediaTrackPrevious"
      KeyEvent.KEYCODE_MEDIA_STOP -> "MediaStop"
      else -> null
    }
    val w = web
    if (name != null && w != null) {
      if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
        w.evaluateJavascript("(()=>{const t=document.activeElement||document.body;t.dispatchEvent(new KeyboardEvent('keydown',{key:'$name',bubbles:true,cancelable:true}))})()", null)
      }
      return true
    }
    return super.dispatchKeyEvent(event)
  }
}
