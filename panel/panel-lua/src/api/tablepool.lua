-- Portions adapted from openresty/lua-tablepool.
-- Upstream source references:
-- https://github.com/openresty/lua-tablepool/blob/94d3a5f43df6526fbaeb0f06873862f964766683/lib/tablepool.lua
--
-- Modifications: Table reuse adapted to Luau table helpers and per-VM pools.
--
-- License source:
-- https://github.com/openresty/lua-tablepool/blob/94d3a5f43df6526fbaeb0f06873862f964766683/README.md#copyright-and-license
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (C) 2016-2021, by Yichun "agentzh" Zhang, OpenResty Inc.
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

-- tablepool: lua-tablepool's pools of tables to reuse, each VM keeping its
-- own.
local clear, create = table.clear, table.create

-- The most tables a pool keeps; those released beyond are left to the
-- collector.
local MOST = 200

local pools = {}

local tablepool = { _VERSION = "0.03" }

function tablepool.fetch(tag, narr, nrec)
    local pool = pools[tag]
    if pool and pool.n > 0 then
        local count = pool.n
        local reused = pool[count]
        pool[count] = nil
        pool.n = count - 1
        return reused
    end
    return create(narr or 0)
end

function tablepool.release(tag, obj, noclear)
    if type(obj) ~= "table" then
        error("object empty", 2)
    end
    local pool = pools[tag]
    if not pool then
        pool = { n = 0 }
        pools[tag] = pool
    end
    if pool.n >= MOST then
        return
    end
    if not noclear then
        clear(obj)
    end
    local count = pool.n + 1
    pool[count] = obj
    pool.n = count
end

return tablepool
