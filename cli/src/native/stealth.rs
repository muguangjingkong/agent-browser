/// Browser Stealth Module
///
/// Makes automated browsers indistinguishable from normal human-operated browsers.
/// Patches all known automation detection vectors used by anti-bot services
/// (Cloudflare, DataDome, PerimeterX, Akamai, reCAPTCHA, etc.).
///
/// Controlled via:
///   - Environment variable: AGENT_BROWSER_STEALTH (default: true)
///   - Launch option: stealth: true/false
///   - CLI flag: --no-stealth

/// Additional Chrome launch args that remove automation signals.
/// These supplement the base args already in chrome.rs.
pub const STEALTH_CHROMIUM_ARGS: &[&str] = &[
    // Core: remove "Chrome is being controlled by automated test software" bar
    "--disable-blink-features=AutomationControlled",
    // Disable info bars / automation warnings
    "--disable-infobars",
    "--disable-notifications",
    // Disable background services that leak automation signals
    "--disable-domain-reliability",
    "--disable-ipc-flooding-protection",
    "--disable-renderer-backgrounding",
    "--disable-breakpad",
    "--disable-component-extensions-with-background-pages",
    "--disable-background-timer-throttling",
    // Prevent detection via client hints
    "--disable-features=AutofillServerCommunication",
    // Disable webdriver-related features
    "--disable-features=Translate,AcceptCHFrame,MediaRouter,OptimizationHints,ProcessPerSiteUpToMainFrameThreshold",
    // Misc
    "--no-service-autorun",
    // Disable WebRTC IP leak (prevents real IP exposure via STUN)
    "--enforce-webrtc-ip-permission-check",
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    // Disable client hints that expose headless signals
    "--disable-features=UserAgentClientHint",
];

/// Comprehensive stealth patches injected via Page.addScriptToEvaluateOnNewDocument.
/// Covers all major detection vectors used by anti-bot services.
pub const STEALTH_INIT_SCRIPT: &str = r#"
// 1. navigator.webdriver — THE primary automation detection signal
Object.defineProperty(navigator, 'webdriver', {
  get: () => undefined,
  configurable: true,
});

// 2. Chrome runtime — real Chrome has window.chrome with specific structure
if (!window.chrome) {
  Object.defineProperty(window, 'chrome', {
    value: {},
    writable: true,
    configurable: true,
  });
}
if (!window.chrome.runtime) {
  Object.defineProperty(window.chrome, 'runtime', {
    value: {
      PNaClEnabled: false,
      onConnect: undefined,
      sendMessage: function() {
        throw new Error('Could not establish connection. Receiving end does not exist.');
      },
      connect: function() { return undefined; },
      id: undefined,
    },
    writable: true,
    configurable: true,
  });
}

// 3. Permissions API — make Notification permission match real behavior
const originalQuery = window.navigator.permissions?.query?.bind(window.navigator.permissions);
if (originalQuery) {
  Object.defineProperty(window.navigator.permissions, 'query', {
    value: function(parameters) {
      if (parameters.name === 'notifications') {
        return Promise.resolve({ state: Notification.permission, onchange: null });
      }
      return originalQuery(parameters);
    },
    writable: true,
    configurable: true,
  });
}

// 4. navigator.plugins — empty in automation, populated in real browser
Object.defineProperty(navigator, 'plugins', {
  get: () => {
    const plugins = [
      { name: 'Chrome PDF Plugin', filename: 'internal-pdf-viewer', description: 'Portable Document Format', length: 1 },
      { name: 'Chrome PDF Viewer', filename: 'mhjfbmdgcfjbbpaeojofohoefgiehjai', description: '', length: 1 },
      { name: 'Native Client', filename: 'internal-nacl-plugin', description: '', length: 2 },
    ];
    plugins.item = (i) => plugins[i] || null;
    plugins.namedItem = (name) => plugins.find(p => p.name === name) || null;
    plugins.refresh = () => {};
    return plugins;
  },
  configurable: true,
});

// 5. navigator.mimeTypes — matches plugins above
Object.defineProperty(navigator, 'mimeTypes', {
  get: () => {
    const mimes = [
      { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format', enabledPlugin: { name: 'Chrome PDF Plugin' } },
      { type: 'application/x-google-chrome-pdf', suffixes: 'pdf', description: 'Portable Document Format', enabledPlugin: { name: 'Chrome PDF Viewer' } },
    ];
    mimes.item = (i) => mimes[i] || null;
    mimes.namedItem = (name) => mimes.find(m => m.type === name) || null;
    return mimes;
  },
  configurable: true,
});

// 6. navigator.languages — empty in some headless configs
Object.defineProperty(navigator, 'languages', {
  get: () => ['en-US', 'en'],
  configurable: true,
});
Object.defineProperty(navigator, 'language', {
  get: () => 'en-US',
  configurable: true,
});

// 7. navigator.hardwareConcurrency — some headless envs report 0 or 1
if (navigator.hardwareConcurrency < 2) {
  Object.defineProperty(navigator, 'hardwareConcurrency', {
    get: () => 4,
    configurable: true,
  });
}

// 8. navigator.deviceMemory — some headless envs don't report this
if (!navigator.deviceMemory) {
  Object.defineProperty(navigator, 'deviceMemory', {
    get: () => 8,
    configurable: true,
  });
}

// 9. WebGL Renderer — hide "SwiftShader" which screams headless
const getParameterOrig = WebGLRenderingContext.prototype.getParameter;
WebGLRenderingContext.prototype.getParameter = function(parameter) {
  if (parameter === 37445) return 'Google Inc. (NVIDIA)';
  if (parameter === 37446) return 'ANGLE (NVIDIA, NVIDIA GeForce GTX 1080 Direct3D11 vs_5_0 ps_5_0, D3D11)';
  return getParameterOrig.call(this, parameter);
};
if (typeof WebGL2RenderingContext !== 'undefined') {
  const getParameter2Orig = WebGL2RenderingContext.prototype.getParameter;
  WebGL2RenderingContext.prototype.getParameter = function(parameter) {
    if (parameter === 37445) return 'Google Inc. (NVIDIA)';
    if (parameter === 37446) return 'ANGLE (NVIDIA, NVIDIA GeForce GTX 1080 Direct3D11 vs_5_0 ps_5_0, D3D11)';
    return getParameter2Orig.call(this, parameter);
  };
}

// 10. window.outerWidth / outerHeight — 0 in headless, should match inner
if (window.outerWidth === 0) {
  Object.defineProperty(window, 'outerWidth', {
    get: () => window.innerWidth,
    configurable: true,
  });
}
if (window.outerHeight === 0) {
  Object.defineProperty(window, 'outerHeight', {
    get: () => window.innerHeight + 85,
    configurable: true,
  });
}

// 11. Connection type — headless often has missing/different networkInfo
if (navigator.connection && navigator.connection.rtt === 0) {
  Object.defineProperty(navigator.connection, 'rtt', {
    get: () => 50,
    configurable: true,
  });
}

// 12. Prevent iframe contentWindow detection of automation
const origAttachShadow = Element.prototype.attachShadow;
if (origAttachShadow) {
  Element.prototype.attachShadow = function(init) {
    return origAttachShadow.call(this, { ...init });
  };
}

// 13. Prevent toString() detection of patched functions
const nativeToString = Function.prototype.toString;
const overrides = new Map();
function patchToString(target, original) {
  overrides.set(target, nativeToString.call(original || target));
}
const originalToString = Function.prototype.toString;
Function.prototype.toString = function() {
  if (overrides.has(this)) return overrides.get(this);
  return originalToString.call(this);
};
patchToString(Function.prototype.toString, originalToString);

// 14. Screen properties — headless may have unrealistic screen values
if (screen.colorDepth < 24) {
  Object.defineProperty(screen, 'colorDepth', { get: () => 24, configurable: true });
}
if (screen.pixelDepth < 24) {
  Object.defineProperty(screen, 'pixelDepth', { get: () => 24, configurable: true });
}

// 15. chrome.csi() — Chrome Speed Insights, checked by DataDome/Cloudflare
if (window.chrome && !window.chrome.csi) {
  window.chrome.csi = function() {
    return {
      onloadT: Date.now(),
      startE: Date.now() - Math.floor(Math.random() * 500 + 100),
      pageT: Math.random() * 2000 + 500,
      tran: 15,
    };
  };
  patchToString(window.chrome.csi, function csi() { return '[native code]'; });
}

// 16. chrome.loadTimes() — legacy timing API, still checked by some services
if (window.chrome && !window.chrome.loadTimes) {
  window.chrome.loadTimes = function() {
    const now = Date.now() / 1000;
    return {
      commitLoadTime: now - Math.random() * 2,
      connectionInfo: 'h2',
      finishDocumentLoadTime: now - Math.random(),
      finishLoadTime: now,
      firstPaintAfterLoadTime: now + Math.random() * 0.1,
      firstPaintTime: now - Math.random() * 0.5,
      navigationType: 'Other',
      npnNegotiatedProtocol: 'h2',
      requestTime: now - Math.random() * 3,
      startLoadTime: now - Math.random() * 2.5,
      wasAlternateProtocolAvailable: false,
      wasFetchedViaSpdy: true,
      wasNpnNegotiated: true,
    };
  };
  patchToString(window.chrome.loadTimes, function loadTimes() { return '[native code]'; });
}

// 17. Notification.permission consistency — must match permissions.query result
if (typeof Notification !== 'undefined' && Notification.permission === 'default') {
  // Real Chrome defaults to 'default', but some headless envs have 'denied'
  // Ensure consistency between Notification.permission and permissions.query()
  try {
    Object.defineProperty(Notification, 'permission', {
      get: () => 'default',
      configurable: true,
    });
  } catch(e) {}
}

// 18. navigator.mediaDevices.enumerateDevices() — empty in headless = bot signal
if (navigator.mediaDevices && navigator.mediaDevices.enumerateDevices) {
  const origEnumerate = navigator.mediaDevices.enumerateDevices.bind(navigator.mediaDevices);
  Object.defineProperty(navigator.mediaDevices, 'enumerateDevices', {
    value: async function() {
      const devices = await origEnumerate();
      if (devices.length === 0) {
        // Return realistic mock devices
        return [
          { deviceId: '', kind: 'audioinput', label: '', groupId: 'default' },
          { deviceId: '', kind: 'videoinput', label: '', groupId: '' },
          { deviceId: '', kind: 'audiooutput', label: '', groupId: 'default' },
        ];
      }
      return devices;
    },
    writable: true,
    configurable: true,
  });
}

// 19. Permissions API — patch all permission types, not just notifications
if (window.navigator.permissions) {
  const origPermQuery = window.navigator.permissions.query.bind(window.navigator.permissions);
  Object.defineProperty(window.navigator.permissions, 'query', {
    value: function(desc) {
      // Return realistic defaults for commonly queried permissions
      const defaults = {
        'notifications': 'default',
        'camera': 'prompt',
        'microphone': 'prompt',
        'geolocation': 'prompt',
        'clipboard-read': 'prompt',
        'clipboard-write': 'granted',
        'midi': 'granted',
        'background-sync': 'granted',
        'persistent-storage': 'prompt',
        'accelerometer': 'granted',
        'gyroscope': 'granted',
        'magnetometer': 'granted',
        'payment-handler': 'granted',
      };
      if (desc && desc.name && defaults[desc.name] !== undefined) {
        return Promise.resolve({ state: defaults[desc.name], onchange: null });
      }
      return origPermQuery(desc).catch(function() {
        return { state: 'prompt', onchange: null };
      });
    },
    writable: true,
    configurable: true,
  });
}

// 20. WebRTC leak protection — prevent IP exposure via RTCPeerConnection
if (typeof RTCPeerConnection !== 'undefined') {
  const origRTC = RTCPeerConnection;
  window.RTCPeerConnection = function(config, constraints) {
    // Force relay-only ICE candidates to prevent local IP leak
    if (config && config.iceServers) {
      config.iceTransportPolicy = 'relay';
    }
    if (!config) config = {};
    config.iceTransportPolicy = 'relay';
    return new origRTC(config, constraints);
  };
  window.RTCPeerConnection.prototype = origRTC.prototype;
  Object.defineProperty(window, 'RTCPeerConnection', { writable: true, configurable: true });
  // Also patch webkitRTCPeerConnection
  if (typeof webkitRTCPeerConnection !== 'undefined') {
    window.webkitRTCPeerConnection = window.RTCPeerConnection;
  }
}

// 21. navigator.getBattery() — missing in some headless = detectable
if (!navigator.getBattery) {
  Object.defineProperty(navigator, 'getBattery', {
    value: function() {
      return Promise.resolve({
        charging: true,
        chargingTime: 0,
        dischargingTime: Infinity,
        level: 1.0,
        addEventListener: function() {},
        removeEventListener: function() {},
      });
    },
    writable: true,
    configurable: true,
  });
}

// 22. Iframe contentWindow webdriver leak — patches webdriver in all frames
try {
  const origCreate = document.createElement.bind(document);
  Object.defineProperty(document, 'createElement', {
    value: function(tagName, options) {
      const el = origCreate(tagName, options);
      if (tagName.toLowerCase() === 'iframe') {
        const origAppend = el.appendChild;
        // After iframe is attached, patch its navigator.webdriver
        const observer = new MutationObserver(function() {
          try {
            if (el.contentWindow) {
              Object.defineProperty(el.contentWindow.navigator, 'webdriver', {
                get: () => undefined,
                configurable: true,
              });
            }
          } catch(e) {}
        });
        // Observe when iframe gets a document
        setTimeout(function patchIframe() {
          try {
            if (el.contentWindow) {
              Object.defineProperty(el.contentWindow.navigator, 'webdriver', {
                get: () => undefined,
                configurable: true,
              });
            }
          } catch(e) {}
        }, 0);
      }
      return el;
    },
    writable: true,
    configurable: true,
  });
} catch(e) {}

// 23. Screen dimensions consistency — match viewport to screen
if (typeof screen !== 'undefined') {
  const w = window.innerWidth || 1920;
  const h = window.innerHeight || 1080;
  if (screen.width < w || screen.width === 0) {
    Object.defineProperty(screen, 'width', { get: () => w, configurable: true });
  }
  if (screen.height < h || screen.height === 0) {
    Object.defineProperty(screen, 'height', { get: () => h + 85, configurable: true });
  }
  if (screen.availWidth < w || screen.availWidth === 0) {
    Object.defineProperty(screen, 'availWidth', { get: () => w, configurable: true });
  }
  if (screen.availHeight < h || screen.availHeight === 0) {
    Object.defineProperty(screen, 'availHeight', { get: () => h + 85, configurable: true });
  }
}

// 24. Performance.now() precision — headless can have microsecond precision (detectable)
// Real browsers round to 100μs due to Spectre mitigations
const origPerfNow = performance.now.bind(performance);
Object.defineProperty(performance, 'now', {
  value: function() {
    return Math.round(origPerfNow() * 10) / 10; // Round to 100μs
  },
  writable: true,
  configurable: true,
});
"#;

/// Realistic user-agent string for macOS (the primary dev platform).
/// Never includes "HeadlessChrome" substring.
pub fn get_realistic_user_agent() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
    }
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_arch = "aarch64")]
        { "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36" }
        #[cfg(not(target_arch = "aarch64"))]
        { "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36" }
    }
    #[cfg(target_os = "windows")]
    {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
    }
}

/// Check if stealth mode should be enabled.
/// Default: true (stealth ON unless explicitly disabled).
pub fn is_stealth_enabled(launch_option: Option<bool>) -> bool {
    // Explicit launch option takes priority
    if let Some(v) = launch_option {
        return v;
    }
    // Environment variable
    if let Ok(env) = std::env::var("AGENT_BROWSER_STEALTH") {
        let lower = env.to_lowercase();
        return lower != "false" && lower != "0";
    }
    // Default: ON
    true
}
