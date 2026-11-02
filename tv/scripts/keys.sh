#!/bin/sh
# Debug: presses remote keys on the attached TV/emulator and prints what has the focus after each.
#   tv/scripts/keys.sh DPAD_DOWN DPAD_LEFT ENTER BACK ...   (KEYCODE_ names without the prefix)
here="$(cd "$(dirname "$0")" && pwd)"; . "$here/env.sh"
for k in "$@"; do
  adb shell input keyevent "KEYCODE_$k"; sleep "${KEY_WAIT:-0.7}"
  printf '%-12s ' "$k"
  node "$here/cdp.cjs" '(()=>{const a=document.activeElement;if(!a||a===document.body)return "(body) "+location.pathname;const r=a.getBoundingClientRect();return a.tagName.toLowerCase()+(a.id?"#"+a.id:"")+(a.className&&typeof a.className==="string"?"."+a.className.split(" ").join("."):"")+" ["+(a.getAttribute("href")||a.dataset.act||a.getAttribute("aria-label")||a.textContent.trim().slice(0,40))+"] @"+Math.round(r.left)+","+Math.round(r.top)+" "+location.pathname+location.search})()'
done
