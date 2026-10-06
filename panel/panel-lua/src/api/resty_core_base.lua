-- resty.core.base as scripts reach for it: table helpers, the status codes
-- lua-resty-core's FFI calls return, the subsystem check and references
-- kept in a table. What hands out FFI buffers is not here: scripts have
-- no FFI to use them with.
local base = {
    version = "0.1.31",
    FFI_OK = 0,
    FFI_ERROR = -1,
    FFI_AGAIN = -2,
    FFI_BUSY = -3,
    FFI_DONE = -4,
    FFI_DECLINED = -5,
    FFI_ABORT = -6,
    FFI_NO_REQ_CTX = -100,
    FFI_BAD_CONTEXT = -101,
}

function base.new_tab(narr)
    return table.create(narr or 0)
end

base.clear_tab = table.clear

-- Slot 0 heads the list of freed slots, as luaL_ref keeps it.
local FREE = 0

function base.ref_in_table(tab, value)
    if value == nil then
        return -1
    end
    local slot = tab[FREE]
    if slot and slot ~= 0 then
        tab[FREE] = tab[slot]
    else
        slot = #tab + 1
    end
    tab[slot] = value
    return slot
end

function base.unref_in_table(tab, slot)
    tab[slot] = tab[FREE]
    tab[FREE] = slot
end

function base.allows_subsystem(...)
    for index = 1, select("#", ...) do
        if select(index, ...) == "http" then
            return
        end
    end
    error("unsupported subsystem: http", 2)
end

return base
