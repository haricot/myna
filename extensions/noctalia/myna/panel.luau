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

  panel.render(ui.column({ gap = 14, flexGrow = 1 }, {
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
  }))
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
end

function onClose()
  opened = false
end

function update()
  if opened and not busy then refresh() end
end

function onStartClicked()
  runServiceAction("start")
end

function onStopClicked()
  runServiceAction("stop")
end

function onRefreshClicked()
  refresh()
end

function onCloseClicked()
  panel.close()
end
