-- Portions adapted from openresty/lua-resty-limit-traffic.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/lib/resty/limit/req.lua
--
-- Modifications: Leaky-bucket accounting adapted to packed Luau strings
-- instead of FFI records.
--
-- License source:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/README.md#copyright-and-license
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (C) 2015-2019, by Yichun "agentzh" Zhang, OpenResty Inc.
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
--
-- The upstream limiter is an approximate port of Nginx's request limiter; its
-- leaky-bucket accounting is retained here.
-- Nginx source reference:
-- https://github.com/nginx/nginx/blob/2b5c2b605b5df669da5dec6749dcc76c07d1315d/src/http/modules/ngx_http_limit_req_module.c
--
-- License source:
-- https://github.com/nginx/nginx/blob/2b5c2b605b5df669da5dec6749dcc76c07d1315d/LICENSE
--
-- Nginx copyright and license (BSD-2-Clause):
--
-- Copyright (C) 2002-2021 Igor Sysoev
-- Copyright (C) 2011-2026 Nginx, Inc.
-- All rights reserved.
--
-- Redistribution and use in source and binary forms, with or without
-- modification, are permitted provided that the following conditions
-- are met:
-- 1. Redistributions of source code must retain the above copyright
--    notice, this list of conditions and the following disclaimer.
-- 2. Redistributions in binary form must reproduce the above copyright
--    notice, this list of conditions and the following disclaimer in the
--    documentation and/or other materials provided with the distribution.
--
-- THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
-- ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
-- IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
-- ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
-- FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
-- DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
-- OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
-- HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
-- LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
-- OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
-- SUCH DAMAGE.

-- resty.limit.req: lua-resty-limit-traffic's leaky bucket limiting the
-- rate of requests per key, its state of each key two numbers packed into
-- an ngx.shared value instead of the C structure the library writes
-- through the FFI.
local abs, max = math.abs, math.max
local pack, unpack = string.pack, string.unpack

-- Excess and the time it was measured, in milli-requests and
-- milliseconds.
local RECORD = "<dd"

local limit = { _VERSION = "0.09" }
local methods = { __index = limit }

function limit.new(dict_name, rate, burst)
    local dict = ngx.shared[dict_name]
    if not dict then
        return nil, "shared dict not found"
    end
    assert(rate > 0 and burst >= 0)
    return setmetatable({
        dict = dict,
        rate = rate * 1000,
        burst = burst * 1000,
    }, methods)
end

local function read(dict, key)
    local record, err = dict:get(key)
    if record == nil then
        return nil, err
    end
    if type(record) ~= "string" or #record ~= 16 then
        return nil, "shdict abused by other users"
    end
    return unpack(RECORD, record)
end

-- The delay before the request with `key` may go on, and the excess per
-- second it leaves.
function limit.incoming(self, key, commit)
    local dict, rate = self.dict, self.rate
    local now = ngx.now() * 1000
    local excess = 0
    local held, last = read(dict, key)
    if held then
        excess = max(held - rate * abs(now - last) / 1000 + 1000, 0)
        if excess > self.burst then
            return nil, "rejected"
        end
    elseif last then
        return nil, last
    end
    if commit then
        local ok, err = dict:set(key, pack(RECORD, excess, now))
        if not ok then
            return nil, err
        end
    end
    return excess / rate, excess / 1000
end

-- Gives back what the last committed request with `key` added.
function limit.uncommit(self, key)
    assert(key)
    local dict = self.dict
    local excess, last = read(dict, key)
    if not excess then
        return nil, last or "not found"
    end
    return dict:set(key, pack(RECORD, max(excess - 1000, 0), last))
end

function limit.set_rate(self, rate)
    self.rate = rate * 1000
end

function limit.set_burst(self, burst)
    self.burst = burst * 1000
end

return limit
