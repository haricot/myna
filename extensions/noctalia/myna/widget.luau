--!nonstrict
-- Noctalia v5.6.0 bar indicator. Read-only status except opening the panel.
-- Never starts a second provider/client and never reads transcript content.

local clientState = "unknown"
local providerState = "unknown"
local querying = false

local function render()
  local ready = clientState == "active" and providerState == "active"
  barWidget.setGlyph(ready and "microphone" or "microphone-off")
  barWidget.setText("Myna")
  barWidget.setTooltip("Myna: client " .. clientState .. ", Candle " .. providerState .. " (cliquer pour le menu)")
end

local function refresh()
  if querying then return end
  querying = true
  local accepted = noctalia.runAsync({
    "systemctl", "--user", "is-active",
    "myna-desktop.service", "myna-candle-provider.service",
  }, function(result)
    querying = false
    -- systemctl is-active exits nonzero when a service is inactive; stdout
    -- still contains an ordered state for each unit. Do not treat it as a
    -- transport error.
    local states = {}
    for line in (result.stdout or ""):gmatch("[^\r\n]+") do
      table.insert(states, line)
    end
    clientState = states[1] or "indisponible"
    providerState = states[2] or "indisponible"
    render()
  end)
  if not accepted then
    querying = false
    clientState = "indisponible"
    providerState = "indisponible"
    render()
  end
end

noctalia.setUpdateInterval(3500)
render()
refresh()

function update()
  refresh()
end

function onClick()
  noctalia.togglePanel("haricot/myna:panel")
end
