-- Portions adapted from openresty/lua-resty-core.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-core/blob/cb7264da6d94c259c4ad01ee5c61b5f65e3c7eb4/lib/resty/core/base.lua
--
-- Modifications: Table references and subsystem checks adapted to Luau; FFI
-- facilities omitted.
--
-- License source:
-- https://github.com/openresty/lua-resty-core/blob/cb7264da6d94c259c4ad01ee5c61b5f65e3c7eb4/README.markdown#copyright-and-license
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (C) 2013-2019, by Yichun "agentzh" Zhang, OpenResty Inc.
--
-- All rights reserved.
--
-- Redistribution and use in source and binary forms, with or without
-- modification, are permitted provided that the following conditions are met:
--
-- * Redistributions of source code must retain the above copyright notice,
-- this list of conditions and the following disclaimer.
--
-- * Redistributions in binary form must reproduce the above copyright notice,
-- this list of conditions and the following disclaimer in the documentation
-- and/or other materials provided with the distribution.
--
-- THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
-- AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
-- IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
-- ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
-- LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
-- CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
-- SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
-- INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
-- CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
-- ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
-- POSSIBILITY OF SUCH DAMAGE.

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
