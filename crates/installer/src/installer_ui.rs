//! Setup and uninstall wizard UI, drawn with the shared Shoji theme.

use shoji_brand::{theme, Accent};

const TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en" data-accent="white">
<head>
  <meta charset="UTF-8">
  <title>Shoji Setup</title>
  <style>
{{THEME_CSS}}
    html, body { height: 100%; overflow: hidden; }
    body { display: flex; flex-direction: column; align-items: center; padding: 44px 44px 32px; user-select: none; font-size: 13px; }
    body::before { content: ""; position: fixed; inset: 0; pointer-events: none;
      background-image: linear-gradient(var(--line) 1px, transparent 1px), linear-gradient(90deg, var(--line) 1px, transparent 1px);
      background-size: 96px 144px; background-position: center -24px;
      -webkit-mask-image: radial-gradient(ellipse 70% 60% at 50% 40%, transparent 45%, #000 100%);
      mask-image: radial-gradient(ellipse 70% 60% at 50% 40%, transparent 45%, #000 100%); opacity: .6; }
    .crane { width: 92px; height: 92px; position: relative; }
    .wordmark { margin-top: 10px; font-family: var(--font-display); font-size: 14px; letter-spacing: .5em; text-indent: .5em; color: var(--ink-2); position: relative; }
    .step { display: none; flex-direction: column; align-items: center; width: 100%; max-width: 380px; margin-top: 30px; flex: 1; position: relative; }
    .step.active { display: flex; }
    h1 { font-family: var(--font-display); font-size: 22px; font-weight: 400; text-align: center; }
    p { margin-top: 8px; color: var(--ink-2); text-align: center; line-height: 1.6; }
    .options { width: 100%; margin-top: 22px; border-radius: 12px; background: var(--surface); border: 1px solid var(--line); }
    label { display: flex; align-items: center; gap: 12px; padding: 12px 16px; border-bottom: 1px solid var(--line); cursor: pointer; }
    label:last-child { border-bottom: 0; }
    input[type=checkbox] { width: 16px; height: 16px; accent-color: var(--deep); }
    .fine { margin-top: 14px; font-size: 11.5px; color: var(--ink-3); text-align: center; line-height: 1.5; }
    .actions { display: flex; gap: 10px; margin-top: auto; padding-top: 20px; }
    button { height: 36px; padding: 0 22px; border-radius: 9px; border: 1px solid var(--line); background: var(--surface-2); color: var(--ink); font: inherit; cursor: pointer; }
    button:hover { border-color: var(--line-strong); }
    button.primary { background: var(--tone); color: var(--on-tone); border-color: transparent; font-weight: 600; }
    button.danger { background: var(--danger); color: #fff; border-color: transparent; font-weight: 600; }
    .track { width: 100%; height: 4px; margin-top: 28px; border-radius: 2px; background: var(--line); overflow: hidden; }
    .track div { height: 100%; width: 0; background: var(--deep); transition: width .2s var(--ease); }
    #status { margin-top: 12px; font-size: 12px; color: var(--ink-3); }
  </style>
</head>
<body>
  {{CRANE}}
  <div class="wordmark">shoji</div>

  <section class="step {{INSTALL_ACTIVE}}" id="install">
    <h1>Install Shoji</h1>
    <p>A paper-thin browser built on the WebView2 engine already in Windows. Installs for you only — no administrator rights needed.</p>
    <div class="options">
      <label><input type="checkbox" id="desktop" checked> Desktop shortcut</label>
      <label><input type="checkbox" id="startMenu" checked> Start menu shortcut</label>
    </div>
    <div class="fine">Free software under the MIT license, provided as is, without warranty.</div>
    <div class="actions">
      <button onclick="post('cancel')">Cancel</button>
      <button class="primary" onclick="install()">Install</button>
    </div>
  </section>

  <section class="step" id="working">
    <h1 id="workingTitle">Installing…</h1>
    <div class="track"><div id="bar"></div></div>
    <div id="status"></div>
  </section>

  <section class="step" id="done">
    <h1 id="doneTitle">Shoji is ready</h1>
    <p id="doneText">Pick a color for your crane the first time you open it.</p>
    <div class="options" id="launchBox"><label><input type="checkbox" id="launch" checked> Open Shoji now</label></div>
    <div class="actions"><button class="primary" onclick="post('finish', { launch: document.getElementById('launch').checked })">Finish</button></div>
  </section>

  <section class="step {{UNINSTALL_ACTIVE}}" id="uninstall">
    <h1>Uninstall Shoji</h1>
    <p>Removes Shoji and its shortcuts from this computer.</p>
    <div class="options"><label><input type="checkbox" id="removeData"> Also remove settings and kept sign-ins</label></div>
    <div class="actions">
      <button onclick="post('cancel')">Cancel</button>
      <button class="danger" onclick="uninstall()">Uninstall</button>
    </div>
  </section>

  <script>
    const $ = id => document.getElementById(id);
    function post(action, payload = {}) {
      if (window.ipc) window.ipc.postMessage(JSON.stringify({ action, ...payload }));
    }
    function show(id) {
      document.querySelectorAll('.step').forEach(s => s.classList.toggle('active', s.id === id));
    }
    function install() {
      show('working');
      post('accept', { desktop: $('desktop').checked, start_menu: $('startMenu').checked });
    }
    function uninstall() {
      $('workingTitle').textContent = 'Removing…';
      show('working');
      post('start_uninstall', { remove_data: $('removeData').checked });
    }
    window.updateProgress = (pct, text) => { $('bar').style.width = pct + '%'; $('status').textContent = text; };
    window.installComplete = () => show('done');
    window.installFailed = text => { $('doneTitle').textContent = 'Setup could not finish'; $('doneText').textContent = text; $('launchBox').hidden = true; show('done'); };
    window.uninstallComplete = () => {
      $('doneTitle').textContent = 'Shoji has been removed';
      $('doneText').textContent = 'Thanks for trying it.';
      $('launchBox').hidden = true;
      show('done');
    };
  </script>
</body>
</html>
"#;

pub fn installer_html(is_uninstall: bool) -> String {
    let (install, uninstall) = if is_uninstall {
        ("", "active")
    } else {
        ("active", "")
    };
    TEMPLATE
        .replace("{{THEME_CSS}}", &theme::css())
        .replace("{{CRANE}}", &theme::crane_svg("c", ""))
        .replace("{{INSTALL_ACTIVE}}", install)
        .replace("{{UNINSTALL_ACTIVE}}", uninstall)
        .replace(
            "data-accent=\"white\"",
            &format!("data-accent=\"{}\"", Accent::default().id()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall_modes() {
        let html = installer_html(false);
        assert!(html.contains(r#"class="step active" id="install""#));
        assert!(html.contains(r#"class="step " id="uninstall""#));
        let html = installer_html(true);
        assert!(html.contains(r#"class="step active" id="uninstall""#));
        assert!(!html.contains("{{"));
    }
}
