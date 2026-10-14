(() => {
  if (window.__mp) return;
  // Управляющие панели (лента, настройки, окно) живут в отдельных окнах снаружи; здесь только вид страницы TikTok.
  const CSS = `
  [class*="DivSideNavContainer"],[class*="DivSideNavPlaceholderContainer"],[class*="DivHeaderContainer"],
  [class*="DivBannerContainer"],[class*="DivDownloadApp"],[class*="DivAppDownload"],
  [data-e2e="top-right-action-bar-get-coin"],[data-e2e="capcut-tag"],[data-e2e="tiktok-logo"],
  [class*="DivCookieBanner"],tiktok-cookie-banner,
  [data-e2e="feed-navigation-prev"],[data-e2e="feed-navigation-next"]
  {display:none!important}
  html.mp-nodesc [class*="DivMediaCardDescriptionContainer"],html.mp-nodesc [class*="DivOverlayBottomLeadingTagsContainer"],
  html.mp-noauthor [class*="DivCreatorInfoContainer"],[data-mp-skip]{display:none!important}
  html.mp-noframe [class*="ArticleItemContainer"]{padding:0!important}
  html.mp-noframe [class*="SectionMediaCardContainer"] [class*="DivContainer"],html.mp-noframe [class*="DivVideoPlayerContainer"],
  html.mp-noframe [class*="DivBasicPlayerWrapper"]{background:transparent!important;border-radius:0!important;box-shadow:none!important}
  html.mp-noframe video{border-radius:0!important}
  html.mp-noframe video{object-fit:contain!important;width:100%!important;height:100%!important}
  html.mp-fill video{object-fit:cover!important}
  html.mp-noframe [class*="SectionMediaCardContainer"],html.mp-noframe [class*="SectionMediaCardContainer"] [class*="DivContainer"],
  html.mp-noframe [class*="DivVideoPlayerContainer"],html.mp-noframe [class*="DivBasicPlayerWrapper"],html.mp-noframe .xgplayer-container,
  html.mp-noframe [class*="DivContentFlexLayout"]{width:100vw!important;height:100vh!important;max-width:none!important;max-height:none!important;margin:0!important}
  html,body{background:#000!important;overflow-x:hidden}
  html{--fo:1;--vo:1}
  #mp-frame{position:fixed;inset:0;border:3px solid #000;opacity:var(--fo);pointer-events:none;z-index:2147483640}
  video{opacity:var(--vo)!important}
  .mp-hint{display:none;position:fixed;left:50%;transform:translateX(-50%);width:96px;height:6px;background:#35e8e0;opacity:.6;border:2px solid #000;z-index:2147483641;pointer-events:none}
  html.mp-ah .mp-hint{display:block}
  .mp-hint.t{top:0;border-top:0}.mp-hint.b{bottom:0;border-bottom:0}
  `;
  const settings = () => Object.assign({ hide_desc: false, hide_author: false, hide_ads: true, no_frame: true, fill_crop: false, fit_window: false, auto_scroll: true }, window.__mpset || {});
  const isAuto = () => settings().auto_scroll !== false;
  function next() {
    const e = document.querySelector('[data-e2e="feed-navigation-next"],[data-e2e="arrow-right"]');
    if (e) e.click(); else window.scrollBy({ top: innerHeight, behavior: 'smooth' });
  }
  function build() {
    if (!document.head || !document.body) return;
    if (!document.getElementById('mp-css')) { const s = document.createElement('style'); s.id = 'mp-css'; s.textContent = CSS; document.head.appendChild(s); }
    if (!document.getElementById('mp-frame')) { const f = document.createElement('div'); f.id = 'mp-frame'; document.body.appendChild(f); for (const c of ['t', 'b']) { const h = document.createElement('div'); h.className = 'mp-hint ' + c; document.body.appendChild(h); } }
  }
  // автопрокрутка: убираем зацикливание и по окончании переходим к следующему видео
  function watch() {
    const auto = isAuto();
    document.querySelectorAll('video').forEach(v => {
      if (!v.__mp) {
        v.__mp = 1;
        v.addEventListener('ended', () => { if (isAuto()) setTimeout(next, 150); });
      }
      if (auto && v.loop) v.loop = false;
      if (!auto && !v.loop) v.loop = true;
    });
  }
  function apply() {
    const st = settings(), c = document.documentElement.classList;
    c.toggle('mp-nodesc', !!st.hide_desc); c.toggle('mp-noauthor', !!st.hide_author);
    c.toggle('mp-noframe', !!st.no_frame); c.toggle('mp-fill', !!st.fill_crop);
  }
  window.__mpApply = apply;
  window.__mpUi = u => {
    const r = document.documentElement.style;
    if (u && u.op_frame !== undefined) r.setProperty('--fo', u.op_frame);
    if (u && u.op_video !== undefined) r.setProperty('--vo', u.op_video);
    document.documentElement.classList.toggle('mp-ah', !u || u.autohide_bar !== false);
  };
  // реклама и рекомендованные LIVE: ролик, который сейчас на экране, сразу пролистываем
  const AD_TEXT = /^(реклама|рекламный пост|sponsored|promoted|ad|paid partnership)$/i;
  const LIVE_TEXT = /^(live|прямой эфир|в эфире)$/i;
  function activeItem() {
    const mid = innerHeight / 2;
    return [...document.querySelectorAll('[data-e2e="recommend-list-item-container"]')].find(e => {
      const r = e.getBoundingClientRect(); return r.top < mid && r.bottom > mid;
    });
  }
  function isAd(it) {
    if (it.querySelector('[class*="AdBadge"],[class*="AdLabel"],[class*="AdTag"],[class*="Sponsor"],[class*="Promoted"],[class*="LiveBadge"],[class*="LiveTag"],[class*="DivLiveCard"],[data-e2e*="live-badge"],a[href*="/live"]')) return true;
    for (const e of it.querySelectorAll('span,div,a,button,p')) {
      if (e.children.length) continue;
      const t = e.textContent.trim();
      if (t.length < 24 && (AD_TEXT.test(t) || LIVE_TEXT.test(t))) return true;
    }
    return false;
  }
  // реклама и рекомендованные LIVE: блок невидим и беззвучен, а когда доходит до экрана — сразу пролистывается.
  // Из ленты блок не убираем (display:none ломает ленту TikTok: она начинает ходить по кругу).
  let lastSkip = 0;
  function skipAds() {
    const on = settings().hide_ads;
    document.querySelectorAll('[data-e2e="recommend-list-item-container"]').forEach(it => {
      const ad = on && isAd(it), hidden = it.dataset.mpAd === '1';
      if (ad && !hidden) {
        it.dataset.mpAd = '1';
        it.style.setProperty('visibility', 'hidden', 'important');
      } else if (!ad && hidden) {
        delete it.dataset.mpAd;
        it.style.removeProperty('visibility');
      }
      if (ad) it.querySelectorAll('video').forEach(v => { try { if (!v.paused) v.pause(); v.muted = true; } catch (e) {} });
    });
    if (on && Date.now() - lastSkip > 700) {
      const a = activeItem();
      if (a && a.dataset.mpAd === '1') { lastSkip = Date.now(); next(); }
    }
  }
  let adT = 0;
  new MutationObserver(() => { if (adT || !settings().hide_ads) return; adT = setTimeout(() => { adT = 0; skipAds(); }, 120); }).observe(document.documentElement, { childList: true, subtree: true });
  let lastFit = '';
  function fit() {
    if (!settings().fit_window || !(window.pywebview && pywebview.api && pywebview.api.fit)) return;
    const v = [...document.querySelectorAll('video')].find(x => { const r = x.getBoundingClientRect(); return r.height > 50 && r.top < innerHeight / 2 && r.bottom > innerHeight / 2; });
    if (!v || !v.videoWidth) return;
    const key = v.videoWidth + 'x' + v.videoHeight + '@' + innerWidth;
    if (key === lastFit) return; lastFit = key;
    pywebview.api.fit(v.videoWidth, v.videoHeight, innerWidth, innerHeight);
  }
  setInterval(fit, 800);
  document.addEventListener('mousedown', e => { if (e.altKey && window.pywebview && pywebview.api) { e.preventDefault(); pywebview.api.drag(); } }, true);
  window.__mp = 1;
  build(); watch(); apply(); skipAds();
  if (window.pywebview && pywebview.api && pywebview.api.ui_state) pywebview.api.ui_state().then(s => window.__mpUi(s.ui));
  setInterval(() => { build(); watch(); apply(); }, 700);
  setInterval(skipAds, 250);
})();
