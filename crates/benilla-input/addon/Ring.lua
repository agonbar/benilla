-- The radial ring: a held binding opens it, the right stick picks an entry by angle, and letting go
-- runs the entry. Written against the 1.12 API. The host (benilla-input) fills BenillaPadRingEntries
-- ({label, command} rows, clockwise from the top) and registers the BenillaPad_* functions.
-- Movement is not touched: it is read from the pad every frame, so a held movement key stays held
-- through the ring.
BINDING_NAME_RADIALRING = "Radial Ring (hold)"

local ring = CreateFrame("Frame", "BenillaPadRing", UIParent)
ring:SetFrameStrata("DIALOG")
ring:SetWidth(320)
ring:SetHeight(320)
ring:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
ring:EnableMouse(false)
ring:Hide()
local shade = ring:CreateTexture(nil, "BACKGROUND")
shade:SetAllPoints(ring)
shade:SetTexture(0, 0, 0, 0.3)

local RADIUS = 120
local labels = {}
local count = 0
local open = false
local slot

local function paint(i, picked)
  if not labels[i] then return end
  if picked then
    labels[i]:SetTexture(0.1, 0.4, 0.8, 0.95)
  else
    labels[i]:SetTexture(0, 0, 0, 0.7)
  end
end

local function build()
  local entries = BenillaPadRingEntries
  if count > 0 or not entries then return end
  count = table.getn(entries)
  for i = 1, count do
    local cell = CreateFrame("Frame", nil, ring)
    cell:SetWidth(104)
    cell:SetHeight(30)
    local angle = (i - 1) * 2 * math.pi / count
    cell:SetPoint("CENTER", ring, "CENTER", RADIUS * math.sin(angle), RADIUS * math.cos(angle))
    labels[i] = cell:CreateTexture(nil, "BACKGROUND")
    labels[i]:SetAllPoints(cell)
    paint(i, false)
    local text = cell:CreateFontString(nil, "OVERLAY")
    text:SetFont("Fonts\\FRIZQT__.TTF", 14)
    text:SetPoint("CENTER", cell, "CENTER", 0, 0)
    text:SetText(entries[i][1])
  end
end

local function close()
  open = false
  if slot then paint(slot, false) end
  slot = nil
  BenillaPad_RingOpen(false)
  ring:Hide()
end

-- The binding's press. Nothing opens in cursor mode, where the right stick is the cursor.
function BenillaPadRingOpen()
  if open or BenillaPad_GetMode() == "cursor" then return end
  build()
  if count == 0 then return end
  open = true
  slot = nil
  BenillaPad_RingOpen(true)
  ring:Show()
end

-- The binding's release: the entry the stick pointed at on the last frame runs once, and a stick
-- at rest runs nothing. A second release finds the ring closed.
function BenillaPadRingRelease()
  if not open then return end
  local picked = slot
  close()
  if picked then RunBinding(BenillaPadRingEntries[picked][2]) end
end

ring:SetScript("OnUpdate", function()
  if not open then return end
  if BenillaPad_GetMode() == "cursor" then
    close()
    return
  end
  local lx, ly, rx, ry = BenillaPad_GetStick()
  local now = BenillaPad_RingSlot(rx, ry, count)
  if now ~= slot then
    if slot then paint(slot, false) end
    if now then paint(now, true) end
    slot = now
  end
end)
