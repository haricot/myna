/* Myna Firefox Navigation R1 — deterministic, bounded, no LLM or eval. */
(function (root) {
  "use strict";
  class MynaIntentRouter {
    static fold(value) {
      return String(value || "").normalize("NFD")
        .replace(/[\u0300-\u036f]/g, "").replace(/[’]/g, "'")
        .toLocaleLowerCase("fr").replace(/\s+/g, " ").trim();
    }
    static route(input) {
      if (typeof input !== "string" || !input.length || input.length > 220 ||
          /[\u0000-\u001f]/.test(input)) return { version: 1, status: "unmatched" };
      const s = this.fold(input).replace(/[.!?;,]+$/g, "").trim();
      const out = action => this.validate(action)
        ? { version: 1, status: "ready", action }
        : { version: 1, status: "unmatched" };
      if (/^(?:affiche|montre|liste|enumere|show|list)(?:[- ]moi)? (?:les |des )?(?:liens|boutons|elements|resultats|links|buttons)$/.test(s)) {
        return out({ type: "list" });
      }
      if (/^(?:descends?|defile(?: vers)? le bas|scroll down|page suivante|plus bas|scroll vers le bas)(?: (?:dans )?la page)?$/.test(s)) {
        return out({ type: "scroll", direction: "down" });
      }
      if (/^(?:remonte|monte|defile(?: vers)? le haut|scroll up|page precedente|plus haut|scroll vers le haut)(?: (?:dans )?la page)?$/.test(s)) {
        return out({ type: "scroll", direction: "up" });
      }
      let m = s.match(/^(?:des que|quand|lorsque) (?:le |la |l')?(?:(bouton|lien|element) )?(.+?) (?:apparait|apparaisse|est disponible|devient disponible|est pret|sera pret)(?:,? (?:clique|appuie|ouvre)(?: dessus| sur (?:lui|elle))?)?$/);
      if (m && /(?:clique|appuie|ouvre)/.test(s.split(m[2]).pop())) {
        return out({ type: "wait_click", target: { kind: "label", role: m[1]==="lien"?"link":m[1]==="bouton"?"button":"any", label: m[2] }, timeout_ms: 8000 });
      }
      m = s.match(/^(?:attends|attendre|wait for|patiente)(?: que)? (?:le |la |l')?(?:(bouton|lien|element) )?(.+?)(?: (?:apparaisse|apparait|soit disponible|soit pret))?$/);
      if (m) return out({ type: "wait", target: { kind: "label", role: m[1]==="lien"?"link":m[1]==="bouton"?"button":"any", label: m[2] }, timeout_ms: 8000 });
      const numbers={ premier:1,premiere:1,deuxieme:2,second:2,seconde:2,troisieme:3,quatrieme:4,cinquieme:5,sixieme:6,septieme:7,huitieme:8,neuvieme:9,dixieme:10 };
      m = s.match(/^(?:clique|appuie|ouvre|click|open)(?: sur)? (?:le |la |l')?(?:(?:lien|bouton|element|resultat)(?: numero | n°? | )?)?(\d{1,2}|premier|premiere|deuxieme|second|seconde|troisieme|quatrieme|cinquieme|sixieme|septieme|huitieme|neuvieme|dixieme)(?: (?:lien|bouton|element|resultat))?$/);
      if (m) return out({ type: "click", target: { kind: "index", index: numbers[m[1]]||Number(m[1]) } });
      m = s.match(/^(?:clique|appuie|click)(?: sur)? (?:le |la |l')?(?:(bouton|lien) )?(.+)$/);
      if (m) return out({ type: "click", target: { kind: "label", role: m[1]==="lien"?"link":m[1]==="bouton"?"button":"any", label: m[2] } });
      m = s.match(/^(?:ouvre|open)(?: le | la | l')?(?:lien |link )(.+)$/);
      if (m) return out({ type: "click", target: { kind: "label", role:"link", label:m[1] } });
      return { version: 1, status:"unmatched" };
    }
    // Stable action contract for a possible future Liquid D1/Mistral decision
    // adapter: a provider may SELECT an approved action, never invent a new op.
    static validate(action) {
      if (!action || Object.getPrototypeOf(action)!==Object.prototype) return false;
      if (action.type==="list") return Object.keys(action).length===1;
      if (action.type==="scroll") return Object.keys(action).length===2 && ["up","down"].includes(action.direction);
      if (!["wait","click","wait_click"].includes(action.type)) return false;
      if (action.type==="wait" || action.type==="wait_click") {
        if (action.timeout_ms!==8000 || Object.keys(action).length!==3) return false;
      } else if (Object.keys(action).length!==2) return false;
      const t=action.target;
      if (!t || Object.getPrototypeOf(t)!==Object.prototype) return false;
      if (t.kind==="index") return action.type==="click" && Object.keys(t).length===2 &&
        Number.isInteger(t.index) && t.index>=1 && t.index<=60;
      return t.kind==="label" && ["any","link","button"].includes(t.role) &&
        Object.keys(t).length===3 && typeof t.label==="string" &&
        t.label.length>=2 && t.label.length<=100 && !/[<>\\\u0000-\u001f]/.test(t.label);
    }
  }
  root.MynaIntentRouter = MynaIntentRouter;
  if (typeof module==="object" && module.exports) module.exports = MynaIntentRouter;
})(globalThis);
