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
    /* A small shoji, lit from behind, with the crane's shadow on its paper (the app icon). It
       glows brighter as setup works, and slides open onto the crane when Shoji is ready. */
    .screen { position: relative; width: 112px; height: 112px; border-radius: 24px; overflow: hidden; flex-shrink: 0;
      box-shadow: 0 0 0 3px #8c7552, 0 10px 30px -12px rgba(60, 40, 10, .45); background: radial-gradient(circle at 50% 42%, #f3ead8, #d9ccb2);
      --paper: #f6efe0; --glow: #fffcf3; --kumiko: rgba(128, 100, 62, .32); --shade: rgb(78, 88, 114); }
    .screen .inside { position: absolute; inset: 14px; display: grid; place-items: center; }
    .screen .inside .crane { width: 84px; height: 84px; opacity: 0; transform: translateY(8px) scale(.9); transition: opacity .6s .5s, transform .8s .5s cubic-bezier(.3, 1.4, .5, 1); }
    .door { position: absolute; top: 0; bottom: 0; width: 50.5%; background-color: var(--paper);
      background-image: radial-gradient(ellipse 120% 90% at var(--gx) 40%, var(--glow), transparent 75%), linear-gradient(var(--kumiko) 1.5px, transparent 1.5px), linear-gradient(90deg, var(--kumiko) 1.5px, transparent 1.5px);
      background-size: 100% 100%, 56px 56px, 56px 56px; background-repeat: no-repeat, repeat, repeat;
      transition: transform 1.2s cubic-bezier(.62, 0, .22, 1), filter .6s; filter: brightness(var(--lit, .94)); }
    .door.l { left: 0; --gx: 100%; background-position: 0 0, 56px 56px, 56px 56px; box-shadow: inset -2px 0 0 rgba(112, 86, 50, .5); }
    .door.r { right: 0; --gx: 0%; background-position: 0 0, 0 56px, 0 56px; box-shadow: inset 2px 0 0 rgba(112, 86, 50, .5); }
    .kage { position: absolute; inset: 15px 10px 13px 16px; filter: blur(2.6px); opacity: .5; transition: opacity .5s, transform 1.4s cubic-bezier(.5, 0, .7, .4);
      --c1: var(--shade); --c2: var(--shade); --c3: var(--shade); --c4: var(--shade); }
    .kage .crane { width: 100%; height: 100%; }
    body.working .kage { animation: kage-breathe 2.4s ease-in-out infinite alternate; }
    @keyframes kage-breathe { to { transform: translateY(-3px); } }
    body.ready .door.l { transform: translateX(-101%); }
    body.ready .door.r { transform: translateX(101%); }
    body.ready .kage { opacity: 0; }
    body.ready .inside .crane { opacity: 1; transform: none; }
    body.flown .kage { opacity: 0; transform: translate(70px, -60px) scale(.6) rotate(-12deg); }
    @media (prefers-reduced-motion: reduce) { .door, .kage, .inside .crane { transition: none !important; animation: none !important; } }
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
  <div class="screen" aria-hidden="true">
    <div class="inside">{{CRANE}}</div>
    <div class="door l"></div><div class="door r"></div>
    <div class="kage">{{KAGE}}</div>
  </div>
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
    <p id="doneText">Come in. The first time you open it, you pick a color for the crane, bring over your bookmarks and history, and make it your default.</p>
    <div class="options" id="launchBox"><label><input type="checkbox" id="launch" checked> Open Shoji now</label></div>
    <div class="actions"><button class="primary" onclick="post('finish', { launch: document.getElementById('launch').checked })">Finish</button></div>
  </section>

  <section class="step {{UNINSTALL_ACTIVE}}" id="uninstall">
    <h1>Uninstall Shoji</h1>
    <p>Removes Shoji, its shortcuts and everything Windows recorded about it from this computer.</p>
    <div class="options"><label><input type="checkbox" id="removeData" checked> Also delete my settings, bookmarks, history and saved sign-ins</label></div>
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
      document.body.classList.add('working');
      show('working');
      post('accept', { desktop: $('desktop').checked, start_menu: $('startMenu').checked });
    }
    function uninstall() {
      $('workingTitle').textContent = 'Removing…';
      document.body.classList.add('working');
      show('working');
      post('start_uninstall', { remove_data: $('removeData').checked });
    }
    window.updateProgress = (pct, text) => {
      $('bar').style.width = pct + '%';
      $('status').textContent = text;
      // The paper brightens as the work goes on.
      document.querySelector('.screen').style.setProperty('--lit', String(.94 + pct / 100 * .1));
    };
    window.installComplete = () => { document.body.classList.remove('working'); document.body.classList.add('ready'); show('done'); };
    window.installFailed = text => { document.body.classList.remove('working'); $('doneTitle').textContent = 'Setup could not finish'; $('doneText').textContent = text; $('launchBox').hidden = true; show('done'); };
    window.uninstallComplete = () => {
      document.body.classList.remove('working');
      document.body.classList.add('flown');
      $('doneTitle').textContent = 'Shoji has been removed';
      $('doneText').textContent = 'The crane has flown. Thank you for the time together.';
      $('launchBox').hidden = true;
      show('done');
    };
    if ({{PURGE}}) { $('removeData').checked = true; uninstall(); }
  </script>
</body>
</html>
"#;

/// `purge` starts removing everything straight away (the user already confirmed in the browser).
pub fn installer_html(is_uninstall: bool, purge: bool) -> String {
    let (install, uninstall) = if is_uninstall {
        ("", "active")
    } else {
        ("active", "")
    };
    TEMPLATE
        .replace("{{PURGE}}", if purge { "true" } else { "false" })
        .replace("{{THEME_CSS}}", &theme::css())
        .replace("{{CRANE}}", &theme::crane_svg("c", ""))
        .replace("{{KAGE}}", &theme::crane_svg("k", ""))
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
        let html = installer_html(false, false);
        assert!(html.contains(r#"class="step active" id="install""#));
        assert!(html.contains(r#"class="step " id="uninstall""#));
        let html = installer_html(true, false);
        assert!(html.contains(r#"class="step active" id="uninstall""#));
        assert!(html.contains("if (false)"));
        assert!(!html.contains("{{"));
        assert!(installer_html(true, true).contains("if (true)"));
    }
}
