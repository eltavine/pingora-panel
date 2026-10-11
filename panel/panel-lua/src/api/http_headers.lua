-- Portions adapted from ledgetech/lua-resty-http.
-- Upstream source references:
-- https://github.com/ledgetech/lua-resty-http/blob/183310324026120ab7eaf5dd82b9be90ae63aadf/lib/resty/http_headers.lua
--
-- Modifications: Case-insensitive header lookup adapted with replacement and
-- deletion handling.
--
-- License source:
-- https://github.com/ledgetech/lua-resty-http/blob/183310324026120ab7eaf5dd82b9be90ae63aadf/LICENSE
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (c) 2013, James Hurst
-- All rights reserved.
--
-- Redistribution and use in source and binary forms, with or without modification,
-- are permitted provided that the following conditions are met:
--
--   Redistributions of source code must retain the above copyright notice, this
--   list of conditions and the following disclaimer.
--
--   Redistributions in binary form must reproduce the above copyright notice, this
--   list of conditions and the following disclaimer in the documentation and/or
--   other materials provided with the distribution.
--
-- THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
-- ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
-- WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
-- DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
-- ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
-- (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
-- LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
-- ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
-- (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
-- SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

-- resty.http_headers: lua-resty-http's header tables, whose names match
-- whatever their case (RFC 9110 §5.1) and keep the case they were set in.
local lower = string.lower

local http_headers = { _VERSION = "0.17.2" }

function http_headers.new()
    -- The name each header was set by, by its lowercase form.
    local names = {}
    return setmetatable({}, {
        __index = function(headers, name)
            if type(name) ~= "string" then
                return nil
            end
            local set = names[lower(name)]
            return set and rawget(headers, set)
        end,
        __newindex = function(headers, name, value)
            local key = lower(name)
            local set = names[key]
            if set then
                rawset(headers, set, nil)
            end
            if value == nil then
                names[key] = nil
                return
            end
            names[key] = name
            rawset(headers, name, value)
        end,
    })
end

return http_headers
