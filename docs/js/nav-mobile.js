// Mobile nav: injects a burger button and slide-down flyout panel so mobile
// visitors can reach every top-level page. Without this, the existing
// @media (max-width:768px) rule .nav-links a:not(.nav-cta):not(.theme-toggle)
// { display:none } leaves only the Download CTA visible on phones, which
// blocks all navigation.
//
// Loads on every page that includes <script src="/js/nav-mobile.js" defer>.
// Runs after DOM parse (defer), injects CSS + burger + panel, wires handlers.
(function () {
  if (window.__keyfireNavMobileMounted) return;
  window.__keyfireNavMobileMounted = true;

  var CSS = [
    ".nav-burger{display:none;background:none;border:0;padding:8px;margin-left:auto;cursor:pointer;color:inherit;-webkit-tap-highlight-color:transparent;}",
    ".nav-burger svg{width:26px;height:26px;display:block;}",
    ".nav-burger:focus-visible{outline:2px solid var(--gold,#f0b942);outline-offset:2px;border-radius:6px;}",
    ".nav-mobile-panel{display:none;position:fixed;top:64px;left:0;right:0;z-index:99;background:rgba(8,11,20,0.96);backdrop-filter:blur(20px) saturate(140%);-webkit-backdrop-filter:blur(20px) saturate(140%);border-top:1px solid var(--hairline,rgba(255,255,255,0.08));padding:8px 24px 24px;max-height:calc(100vh - 64px);overflow-y:auto;}",
    "[data-theme=\"light\"] .nav-mobile-panel{background:rgba(255,255,255,0.98);border-top-color:rgba(0,0,0,0.08);}",
    ".nav-mobile-panel.open{display:block;}",
    ".nav-mobile-panel a{display:block;padding:16px 4px;font-size:16px;font-weight:500;color:var(--text,#e8e8ec);text-decoration:none;border-bottom:1px solid var(--hairline,rgba(255,255,255,0.06));letter-spacing:0.01em;}",
    "[data-theme=\"light\"] .nav-mobile-panel a{color:var(--text,#0d0d11);border-bottom-color:rgba(0,0,0,0.06);}",
    ".nav-mobile-panel a:last-child{border-bottom:0;}",
    ".nav-mobile-panel a.nav-cta{background:linear-gradient(180deg,var(--gold-light,#ffbb44),var(--gold,#f0b942));color:#0d0d11;border:1px solid var(--gold,#f0b942);border-radius:10px;padding:14px 18px;font-weight:600;text-align:center;margin-top:14px;box-shadow:0 1px 6px rgba(232,160,32,0.35),inset 0 1px 0 rgba(255,255,255,0.18);}",
    "[data-theme=\"light\"] .nav-mobile-panel a.nav-cta{color:#fff;}",
    "body.nav-mobile-open{overflow:hidden;}",
    "@media (max-width:768px){",
    "  .nav-burger{display:block;}",
    "  nav .nav-links .nav-cta{display:none!important;}",
    "  nav .nav-links{gap:0;}",
    "}"
  ].join("\n");

  var style = document.createElement("style");
  style.setAttribute("data-nav-mobile", "");
  style.textContent = CSS;
  document.head.appendChild(style);

  function init() {
    var nav = document.querySelector("nav");
    if (!nav) return;
    var links = nav.querySelector(".nav-links");
    if (!links) return;
    if (nav.querySelector(".nav-burger")) return;

    var burger = document.createElement("button");
    burger.className = "nav-burger";
    burger.type = "button";
    burger.setAttribute("aria-label", "Open menu");
    burger.setAttribute("aria-expanded", "false");
    burger.setAttribute("aria-controls", "nav-mobile-panel");
    burger.innerHTML =
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="3" y1="6" x2="21" y2="6"></line><line x1="3" y1="12" x2="21" y2="12"></line><line x1="3" y1="18" x2="21" y2="18"></line></svg>';
    nav.appendChild(burger);

    var panel = document.createElement("div");
    panel.className = "nav-mobile-panel";
    panel.id = "nav-mobile-panel";
    panel.setAttribute("role", "menu");

    Array.prototype.forEach.call(links.querySelectorAll("a"), function (a) {
      var clone = a.cloneNode(true);
      clone.setAttribute("role", "menuitem");
      clone.removeAttribute("class");
      if (a.classList.contains("nav-cta")) clone.className = "nav-cta";
      panel.appendChild(clone);
    });

    document.body.appendChild(panel);

    function closePanel() {
      panel.classList.remove("open");
      document.body.classList.remove("nav-mobile-open");
      burger.setAttribute("aria-expanded", "false");
      burger.setAttribute("aria-label", "Open menu");
    }

    function openPanel() {
      panel.classList.add("open");
      document.body.classList.add("nav-mobile-open");
      burger.setAttribute("aria-expanded", "true");
      burger.setAttribute("aria-label", "Close menu");
    }

    burger.addEventListener("click", function () {
      if (panel.classList.contains("open")) closePanel();
      else openPanel();
    });

    panel.addEventListener("click", function (e) {
      var t = e.target;
      while (t && t !== panel) {
        if (t.tagName === "A") {
          closePanel();
          return;
        }
        t = t.parentNode;
      }
    });

    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && panel.classList.contains("open")) closePanel();
    });

    var mq = window.matchMedia("(min-width: 769px)");
    var onChange = function (ev) {
      if (ev.matches) closePanel();
    };
    if (mq.addEventListener) mq.addEventListener("change", onChange);
    else if (mq.addListener) mq.addListener(onChange);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
