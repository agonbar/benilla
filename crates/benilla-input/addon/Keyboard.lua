-- The on-screen keyboard: keys are buttons, so the pad's cursor clicks them and the d-pad snaps from
-- key to key. A key types through BenillaPad_Type, which the host sends as the keyboard text a
-- physical keyboard makes, so it reaches the focused EditBox. Written against the 1.12 API.
-- Shown by hand: a long press of the pad's view button, the button by the chat frame, or the
-- TOGGLEPADKEYBOARD binding.
BINDING_NAME_TOGGLEPADKEYBOARD = "Toggle Pad Keyboard"

local KEY = 46
local GAP = 4
local LAYERS = {
  lower = { "1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,./" },
  upper = { "!@#$%^&*()", "QWERTYUIOP", "ASDFGHJKL\"", "ZXCVBNM<>?" },
  symbols = { "1234567890", "-_=+[]{}\\|", ";:~`^*()@#", "!$%&<>?/.," },
}

local layer = "lower"
local keys = {}
local shiftText, symbolText

local frame = CreateFrame("Frame", "BenillaPadKeyboard", UIParent)
frame:SetFrameStrata("FULLSCREEN_DIALOG")
frame:SetWidth(10 * (KEY + GAP) + GAP)
frame:SetHeight(5 * (KEY + GAP) + GAP)
frame:SetPoint("BOTTOM", UIParent, "BOTTOM", 0, 24)
frame:EnableMouse(true)
frame:Hide()
local shade = frame:CreateTexture(nil, "BACKGROUND")
shade:SetAllPoints(frame)
shade:SetTexture(0, 0, 0, 0.85)

-- The keyboard closes with Escape like any other window.
if UISpecialFrames then table.insert(UISpecialFrames, "BenillaPadKeyboard") end

local function relabel()
  local rows = LAYERS[layer]
  for r = 1, 4 do
    for c = 1, 10 do
      keys[r][c].text:SetText(string.sub(rows[r], c, c))
      keys[r][c].char = string.sub(rows[r], c, c)
    end
  end
  shiftText:SetText(layer == "upper" and "SHIFT" or "Shift")
  symbolText:SetText(layer == "symbols" and "abc" or "123")
end

-- A button named `name` of `units` key widths at column `x`, row `row` (1 is the top).
local function makeKey(name, x, row, units, label, onclick)
  local b = CreateFrame("Button", name, frame)
  b:SetWidth(units * (KEY + GAP) - GAP)
  b:SetHeight(KEY)
  b:SetPoint("TOPLEFT", frame, "TOPLEFT", GAP + x * (KEY + GAP), -(GAP + (row - 1) * (KEY + GAP)))
  local bg = b:CreateTexture(nil, "BACKGROUND")
  bg:SetAllPoints(b)
  bg:SetTexture(0.18, 0.18, 0.18, 0.95)
  b.text = b:CreateFontString(nil, "OVERLAY")
  b.text:SetFont("Fonts\\FRIZQT__.TTF", 16)
  b.text:SetPoint("CENTER", b, "CENTER", 0, 0)
  b.text:SetText(label)
  -- The pad's A is a right click and its X a left one; both type.
  b:RegisterForClicks("LeftButtonUp", "RightButtonUp")
  b:SetScript("OnEnter", function() bg:SetTexture(0.1, 0.4, 0.8, 0.95) end)
  b:SetScript("OnLeave", function() bg:SetTexture(0.18, 0.18, 0.18, 0.95) end)
  b:SetScript("OnClick", onclick)
  return b
end

local function typeChar(b)
  if b.char and b.char ~= "" then BenillaPad_Type(b.char) end
  -- Shift is for the next character only.
  if layer == "upper" then
    layer = "lower"
    relabel()
  end
end

for r = 1, 4 do
  keys[r] = {}
  for c = 1, 10 do
    local b
    b = makeKey("BenillaPadKey" .. r .. "_" .. c, c - 1, r, 1, "", function() typeChar(b) end)
    keys[r][c] = b
  end
end

local shift = makeKey("BenillaPadKeyShift", 0, 5, 1.5, "Shift", function()
  if layer == "upper" then layer = "lower" else layer = "upper" end
  relabel()
end)
shiftText = shift.text
local symbols = makeKey("BenillaPadKeySymbols", 1.5, 5, 1.5, "123", function()
  if layer == "symbols" then layer = "lower" else layer = "symbols" end
  relabel()
end)
symbolText = symbols.text
makeKey("BenillaPadKeySpace", 3, 5, 3, "Space", function() BenillaPad_Type(" ") end)
makeKey("BenillaPadKeyBack", 6, 5, 1.5, "Back", function() BenillaPad_Key("BACKSPACE") end)
makeKey("BenillaPadKeyEnter", 7.5, 5, 1.5, "Enter", function() BenillaPad_Key("ENTER") end)
makeKey("BenillaPadKeyClose", 9, 5, 1, "X", function() frame:Hide() end)
relabel()

function BenillaPadShowKeyboard()
  if not frame:IsShown() then frame:Show() end
end

function BenillaPadToggleKeyboard()
  if frame:IsShown() then frame:Hide() else frame:Show() end
end

-- A button by the chat frame, for the controller presentation: it opens the chat line and the keyboard.
local chatButton
local function chatKeyboard()
  if not chatButton then
    if not ChatFrame1 then return end
    chatButton = CreateFrame("Button", "BenillaPadChatKeyboard", UIParent)
    chatButton:SetWidth(34)
    chatButton:SetHeight(24)
    chatButton:SetPoint("BOTTOMRIGHT", ChatFrame1, "TOPRIGHT", 0, 4)
    local bg = chatButton:CreateTexture(nil, "BACKGROUND")
    bg:SetAllPoints(chatButton)
    bg:SetTexture(0, 0, 0, 0.6)
    local text = chatButton:CreateFontString(nil, "OVERLAY")
    text:SetFont("Fonts\\FRIZQT__.TTF", 12)
    text:SetPoint("CENTER", chatButton, "CENTER", 0, 0)
    text:SetText("Kbd")
    chatButton:RegisterForClicks("LeftButtonUp", "RightButtonUp")
    chatButton:SetScript("OnClick", function()
      if ChatFrame_OpenChat then ChatFrame_OpenChat("") end
      BenillaPadShowKeyboard()
    end)
  end
  local want = BenillaPadState and BenillaPadState.presentation == "controller"
  if want and not chatButton:IsShown() then chatButton:Show() end
  if not want and chatButton:IsShown() then chatButton:Hide() end
end

local watcher = CreateFrame("Frame", "BenillaPadKeyboardWatcher", UIParent)
local since = 0
watcher:SetScript("OnUpdate", function()
  since = since + arg1
  if since < 0.25 then return end
  since = 0
  chatKeyboard()
end)
