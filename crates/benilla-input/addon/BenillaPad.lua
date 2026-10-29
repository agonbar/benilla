-- Benilla Pad: the bars show pad glyphs, and the pad's cursor is drawn.
-- Written against the 1.12 API. benilla-input fills BenillaPadState (mode, cursor, presentation),
-- BenillaPadGlyphs (token to label) and BenillaPadLayers (modifier to label) about ten times a second.

-- The two commands of Bindings.xml. A body only asks; benilla-input takes the asks about once a
-- frame, because whether there is something to interact with is the soft target's to say.
BINDING_HEADER_BENILLAPAD = "Benilla Pad"
BINDING_NAME_INTERACTTARGET = "Interact with Target"
BINDING_NAME_INTERACTORJUMP = "Interact or Jump"
BenillaPadAsks = nil
function BenillaPadAsk(what)
  BenillaPadAsks = (BenillaPadAsks or "") .. what .. " "
end

local GetBindingTextStock = GetBindingText

local function controller()
  return BenillaPadState and BenillaPadState.presentation == "controller"
end

-- "SHIFT-PAD1" to {"SHIFT"}, "PAD1". A trailing hyphen is the key "-" itself.
local function split(name)
  local mods = {}
  local rest = name
  while true do
    local i = string.find(rest, "-", 1, true)
    if not i or i == string.len(rest) then break end
    table.insert(mods, string.sub(rest, 1, i - 1))
    rest = string.sub(rest, i + 1)
  end
  return mods, rest
end

-- The pad's label for a binding name, or nil when it is not a pad binding with a label.
local function padText(name)
  if not name or not BenillaPadGlyphs then return nil end
  local mods, base = split(name)
  local glyph = BenillaPadGlyphs[base]
  if not glyph then return nil end
  local text = glyph
  for i = table.getn(mods), 1, -1 do
    text = (BenillaPadLayers and BenillaPadLayers[mods[i]] or mods[i]) .. "-" .. text
  end
  return text
end

-- Bar hotkeys and the Key Bindings window read this; a keyboard binding reads as stock.
function GetBindingText(name, prefix, abbreviate)
  if controller() then
    local text = padText(name)
    if text then return text end
  end
  return GetBindingTextStock(name, prefix, abbreviate)
end

local BARS = {
  { "ActionButton", "ACTIONBUTTON" },
  { "MultiBarBottomLeftButton", "MULTIACTIONBAR1BUTTON" },
  { "MultiBarBottomRightButton", "MULTIACTIONBAR2BUTTON" },
  { "MultiBarRightButton", "MULTIACTIONBAR3BUTTON" },
  { "MultiBarLeftButton", "MULTIACTIONBAR4BUTTON" },
}

-- The key a button shows: a pad key in controller presentation, else a keyboard one, else any.
local function pick(k1, k2)
  local keys = { k1, k2 }
  local want = controller()
  for i = 1, 2 do
    local k = keys[i]
    if k and ((string.find(k, "PAD", 1, true) ~= nil) == want) then return k end
  end
  return k1
end

local function refreshHotkeys()
  for b = 1, table.getn(BARS) do
    for i = 1, 12 do
      local hotkey = getglobal(BARS[b][1] .. i .. "HotKey")
      if hotkey then
        local key = pick(GetBindingKey(BARS[b][2] .. i))
        if key then
          local text = GetBindingText(key, "KEY_", 1)
          if hotkey:GetText() ~= text then hotkey:SetText(text) end
        end
      end
    end
  end
end

-- The virtual cursor: the arrow, at the cursor the host moves, while the pad holds the cursor.
local cursor = CreateFrame("Frame", "BenillaPadCursor", UIParent)
cursor:SetFrameStrata("TOOLTIP")
cursor:SetWidth(32)
cursor:SetHeight(32)
cursor:EnableMouse(false)
local arrow = cursor:CreateTexture(nil, "OVERLAY")
arrow:SetAllPoints(cursor)
arrow:SetTexture("Interface\\Cursor\\Point")
cursor:Hide()

local driver = CreateFrame("Frame", "BenillaPadDriver", UIParent)
local since = 0
local shownPresentation
driver:SetScript("OnUpdate", function()
  local show = BenillaPadState and BenillaPadState.cursor == 1
  if show then
    local x, y = GetCursorPosition()
    local scale = UIParent:GetEffectiveScale()
    cursor:ClearAllPoints()
    cursor:SetPoint("TOPLEFT", UIParent, "BOTTOMLEFT", x / scale, y / scale)
    if not cursor:IsShown() then cursor:Show() end
  elseif cursor:IsShown() then
    cursor:Hide()
  end
  since = since + arg1
  if since < 0.25 then return end
  since = 0
  refreshHotkeys()
end)
