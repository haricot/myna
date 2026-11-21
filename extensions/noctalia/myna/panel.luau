--!nonstrict
-- Noctalia v5.6.0 Myna control panel.
-- Fixed argv only: no shell, sudo, model download, or arbitrary commands.
-- systemd --user remains the owner of both service lifecycles.

local clientState = "unknown"
local providerState = "unknown"
local opened = false
local checking = false
local busy = false
local feedback = ""
local control = noctalia.expandPath("~/.local/bin/myna-control")
local captureMode = "unknown"       -- smart or manual
local recognitionMode = "unknown"   -- streaming, offline, hybrid
local checkingSettings = false
local validCapture = {smart=true, manual=true}
local validRecognition = {streaming=true, offline=true, hybrid=true}

local function firstWord(s)
  return (s or ""):match("^%s*([%w_-]+)") or ""
end

local function isReady()
  return clientState == "active" and providerState == "active"
end

local function isPartlyActive()
  return clientState == "active" or providerState == "active"
end

local function render()
  if not opened then return end
  local ready = isReady()
  local status = ready and "Prêt" or (isPartlyActive() and "Partiellement démarré" or "Arrêté")
  local statusColor = ready and "primary" or "on_surface_variant"

  panel.render(ui.scroll({ flexGrow = 1 }, { ui.column({ gap = 14 }, {
    ui.row({ align = "center", justify = "space_between", gap = 8 }, {
      ui.label({
        text = "Myna — Dictée locale",
        fontSize = 16,
        fontWeight = "bold",
        color = "on_surface",
        flexGrow = 1,
      }),
      ui.button({ glyph = "close", variant = "ghost", onClick = "onCloseClicked" }),
    }),
    ui.label({ text = status, color = statusColor, fontWeight = "medium" }),
    ui.separator({ spacing = 2 }),
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({ text = "Candle · Nemotron" }),
      ui.label({ text = providerState, color = "on_surface_variant" }),
    }),
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({ text = "Client · Myna" }),
      ui.label({ text = clientState, color = "on_surface_variant" }),
    }),
    ui.row({ gap = 10, align = "center" }, {
      ui.button({
        text = "Démarrer Myna",
        glyph = "player-play",
        variant = "primary",
        flexGrow = 1,
        enabled = not busy and not ready,
        onClick = "onStartClicked",
      }),
      ui.button({
        text = "Arrêter Myna",
        glyph = "player-stop",
        variant = "destructive",
        flexGrow = 1,
        enabled = not busy and isPartlyActive(),
        onClick = "onStopClicked",
      }),
    }),
    ui.separator({ spacing = 2 }),
    ui.label({
      text = "Reconnaissance : " .. (
        recognitionMode == "streaming" and "Streaming" or
        recognitionMode == "offline" and "Offline" or
        recognitionMode == "hybrid" and "Hybride" or "Indisponible"
      ),
      fontWeight = "medium",
    }),
    ui.row({ gap = 5 }, {
      ui.button({
        text = "Streaming", controlSize = "sm",
        flexGrow = 1, selected = recognitionMode == "streaming",
        enabled = not busy and recognitionMode ~= "streaming",
        onClick = "onStreamingClicked",
      }),
      ui.button({
        text = "Offline", controlSize = "sm",
        flexGrow = 1, selected = recognitionMode == "offline",
        enabled = not busy and recognitionMode ~= "offline",
        onClick = "onOfflineClicked",
      }),
      ui.button({
        text = "Hybride", controlSize = "sm",
        flexGrow = 1, selected = recognitionMode == "hybrid",
        enabled = not busy and recognitionMode ~= "hybrid",
        onClick = "onHybridClicked",
      }),
    }),
    ui.label({
      text = "Hybride : transcription provisoire, puis résultat final hors ligne.",
      color = "on_surface_variant", fontSize = 12, maxLines = 2,
    }),
    ui.separator({ spacing = 2 }),
    ui.label({
      text = "Capture : " .. (
        captureMode == "smart" and "SmartTurn automatique" or
        captureMode == "manual" and "Manuel continu" or "Indisponible"
      ),
      fontWeight = "medium",
    }),
    ui.row({ gap = 6 }, {
      ui.button({
        text = "SmartTurn (auto)", controlSize = "sm",
        flexGrow = 1, selected = captureMode == "smart",
        enabled = not busy and captureMode ~= "smart",
        onClick = "onSmartClicked",
      }),
      ui.button({
        text = "Manuel continu", controlSize = "sm",
        flexGrow = 1, selected = captureMode == "manual",
        enabled = not busy and captureMode ~= "manual",
        onClick = "onManualClicked",
      }),
    }),
    ui.label({
      text = "Manuel : second appui sur Ctrl+Alt+M pour arrêter.",
      color = "on_surface_variant", fontSize = 12, maxLines = 2,
    }),
    ui.separator({ spacing = 2 }),
    ui.button({
      text = "Actualiser l'état",
      glyph = "refresh",
      variant = "ghost",
      enabled = not busy,
      onClick = "onRefreshClicked",
    }),
    ui.label({
      text = feedback,
      color = "on_surface_variant",
      fontSize = 12,
      maxLines = 2,
    }),
  }) }))
end

local function refresh()
  if checking then return end
  checking = true
  local accepted = noctalia.runAsync({
    "systemctl", "--user", "is-active",
    "myna-desktop.service", "myna-candle-provider.service",
  }, function(result)
    checking = false
    local states = {}
    for line in (result.stdout or ""):gmatch("[^\r\n]+") do
      table.insert(states, line)
    end
    clientState = states[1] or "indisponible"
    providerState = states[2] or "indisponible"
    render()
  end)
  if not accepted then
    checking = false
    clientState = "indisponible"
    providerState = "indisponible"
    feedback = "Impossible d'interroger systemd utilisateur."
    render()
  end
end

local function refreshSettings()
  if checkingSettings or busy then return end
  checkingSettings = true
  local ok = noctalia.runAsync({control, "recognition"}, function(result)
    local value = firstWord(result.stdout)
    recognitionMode = (result.exitCode == 0 and validRecognition[value]) and value or "unknown"
    local ok2 = noctalia.runAsync({control, "mode"}, function(res)
      checkingSettings = false
      local mode = firstWord(res.stdout)
      captureMode = (res.exitCode == 0 and validCapture[mode]) and mode or "unknown"
      render()
    end)
    if not ok2 then
      checkingSettings = false
      captureMode = "unknown"
      render()
    end
  end)
  if not ok then
    checkingSettings = false
    recognitionMode = "unknown"
    captureMode = "unknown"
    render()
  end
end

local function settingAction(kind, value)
  if busy then return end
  if kind == "mode" then
    if not validCapture[value] or captureMode == value then return end
  elseif kind == "recognition" then
    if not validRecognition[value] or recognitionMode == value then return end
  else
    return
  end
  busy = true
  feedback = "Application du réglage… la dictée peut être interrompue."
  render()
  -- Fixed allowed values; do not pass transcriptions or arbitrary commands.
  local ok = noctalia.runAsync({control, kind .. "-apply", value}, function(result)
    busy = false
    feedback = (result.exitCode == 0 and not result.timedOut)
      and ("Réglage appliqué : " .. value)
      or "Échec du réglage : vérifier myna-control et les journaux."
    refreshSettings()
    render()
  end)
  if not ok then
    busy = false
    feedback = "Impossible de lancer myna-control."
    render()
  end
end

local function runServiceAction(verb)
  if busy or (verb ~= "start" and verb ~= "stop") then return end
  busy = true
  feedback = (verb == "start") and "Démarrage en cours…" or "Arrêt en cours…"
  render()
  local accepted = noctalia.runAsync({
    "systemctl", "--user", verb,
    "myna-desktop.service", "myna-candle-provider.service",
  }, function(result)
    busy = false
    if result.exitCode == 0 and not result.timedOut then
      feedback = (verb == "start") and "Myna a été démarré." or "Myna a été arrêté."
    else
      feedback = "Échec systemd : consulter journalctl --user -u myna-desktop.service"
    end
    refresh()
    refreshSettings()
    render()
  end)
  if not accepted then
    busy = false
    feedback = "Impossible de lancer systemctl."
    render()
  end
end

noctalia.setUpdateInterval(3500)

function onOpen(_context)
  opened = true
  feedback = ""
  render()
  refresh()
  refreshSettings()
end

function onClose()
  opened = false
end

function update()
  if opened and not busy then
    refresh()
    refreshSettings()
  end
end

function onStartClicked()
  runServiceAction("start")
end

function onStopClicked()
  runServiceAction("stop")
end

function onRefreshClicked()
  refresh()
  refreshSettings()
end

function onStreamingClicked() settingAction("recognition", "streaming") end
function onOfflineClicked() settingAction("recognition", "offline") end
function onHybridClicked() settingAction("recognition", "hybrid") end
function onSmartClicked() settingAction("mode", "smart") end
function onManualClicked() settingAction("mode", "manual") end

function onCloseClicked()
  panel.close()
end
