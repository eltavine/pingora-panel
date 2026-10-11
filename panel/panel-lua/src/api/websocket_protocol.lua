-- Portions adapted from openresty/lua-resty-websocket.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-websocket/blob/45d8e096d886c6debd0af31d3988ba90de695d72/lib/resty/websocket/protocol.lua
--
-- Modifications: Frame encoding and decoding adapted to bit32 and Luau buffers
-- instead of FFI.
--
-- License source:
-- https://github.com/openresty/lua-resty-websocket/blob/45d8e096d886c6debd0af31d3988ba90de695d72/README.markdown#copyright-and-license
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (C) 2013-2017, by Yichun Zhang (agentzh) <agentzh@gmail.com>,
-- OpenResty Inc.
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

-- resty.websocket.protocol: WebSocket frames (RFC 6455 §5) as
-- lua-resty-websocket reads and writes them, in plain Luau, so that its
-- server and client need no FFI.
local band, bor, bxor = bit32.band, bit32.bor, bit32.bxor
local lshift, rshift = bit32.lshift, bit32.rshift
local byte, char, sub = string.byte, string.char, string.sub
local random = require "resty.random"

local protocol = { version = "0.13" }

local TYPES = {
    [0x0] = "continuation",
    [0x1] = "text",
    [0x2] = "binary",
    [0x8] = "close",
    [0x9] = "ping",
    [0xa] = "pong",
}

-- `data` XORed with the four bytes of `key` (RFC 6455 §5.3), four at a time
-- where Luau's buffers allow it.
local function mask(data, key)
    local length = #data
    if length == 0 then
        return data
    end
    local source = buffer.fromstring(data)
    local masked = buffer.create(length)
    local word = buffer.readu32(buffer.fromstring(key), 0)
    local whole = length - length % 4
    for at = 0, whole - 4, 4 do
        buffer.writeu32(masked, at, bxor(buffer.readu32(source, at), word))
    end
    for at = whole, length - 1 do
        buffer.writeu8(masked, at, bxor(buffer.readu8(source, at), byte(key, at % 4 + 1)))
    end
    return buffer.tostring(masked)
end

local function failed(what, err)
    return nil, nil, "failed to receive " .. what .. ": " .. tostring(err)
end

-- The next frame on `sock`: its payload, its type and, for a close frame,
-- the status code, or "again" for a frame that more frames continue.
function protocol.recv_frame(sock, max_payload_len, force_masking)
    local head, err = sock:receive(2)
    if not head then
        return failed("the first 2 bytes", err)
    end
    local first, second = byte(head, 1, 2)
    local fin = band(first, 0x80) ~= 0
    if band(first, 0x70) ~= 0 then
        return nil, nil, "bad RSV1, RSV2, or RSV3 bits"
    end
    local opcode = band(first, 0x0f)
    if opcode >= 0x3 and opcode <= 0x7 then
        return nil, nil, "reserved non-control frames"
    end
    if opcode >= 0xb then
        return nil, nil, "reserved control frames"
    end
    local masked = band(second, 0x80) ~= 0
    if force_masking and not masked then
        return nil, nil, "frame unmasked"
    end
    local length = band(second, 0x7f)
    if length == 126 then
        local extended
        extended, err = sock:receive(2)
        if not extended then
            return failed("the 2 byte payload length", err)
        end
        length = bor(lshift(byte(extended, 1), 8), byte(extended, 2))
    elseif length == 127 then
        local extended
        extended, err = sock:receive(8)
        if not extended then
            return failed("the 8 byte payload length", err)
        end
        local b1, b2, b3, b4, b5, b6, b7, b8 = byte(extended, 1, 8)
        if b1 ~= 0 or b2 ~= 0 or b3 ~= 0 or b4 >= 0x20 then
            return nil, nil, "payload len too large"
        end
        length = ((((b4 * 256 + b5) * 256 + b6) * 256 + b7) * 256) + b8
    end
    if band(opcode, 0x8) ~= 0 then
        if length > 125 then
            return nil, nil, "too long payload for control frame"
        end
        if not fin then
            return nil, nil, "fragmented control frame"
        end
    end
    if length > max_payload_len then
        return nil, nil, "exceeding max payload len"
    end
    local key
    if masked then
        key, err = sock:receive(4)
        if not key then
            return failed("the 4 byte masking key", err)
        end
    end
    local payload = ""
    if length > 0 then
        payload, err = sock:receive(length)
        if not payload then
            return failed("the payload data", err)
        end
    end
    if masked then
        payload = mask(payload, key)
    end
    if opcode == 0x8 then
        if length == 0 then
            return "", "close", nil
        end
        if length == 1 then
            return nil, nil, "bad close frame: the status code takes two bytes"
        end
        return sub(payload, 3), "close", bor(lshift(byte(payload, 1), 8), byte(payload, 2))
    end
    return payload, TYPES[opcode], (not fin) and "again" or nil
end

-- The bytes of a frame, its payload masked with a random key when
-- `masking` (clients must, RFC 6455 §5.1).
local function build_frame(fin, opcode, payload_len, payload, masking)
    local first = bor(fin and 0x80 or 0, opcode)
    local second, extended
    if payload_len <= 125 then
        second, extended = payload_len, ""
    elseif payload_len <= 0xffff then
        second, extended = 126, char(rshift(payload_len, 8), band(payload_len, 0xff))
    elseif payload_len <= 0x7fffffff then
        second = 127
        extended = char(
            0, 0, 0, 0,
            band(rshift(payload_len, 24), 0xff),
            band(rshift(payload_len, 16), 0xff),
            band(rshift(payload_len, 8), 0xff),
            band(payload_len, 0xff)
        )
    else
        return nil, "payload too big"
    end
    if not masking then
        return char(first, second) .. extended .. payload
    end
    local key = random.bytes(4)
    return char(first, bor(second, 0x80)) .. extended .. key .. mask(payload, key)
end

protocol.build_frame = build_frame

-- Sends a frame of `payload` on `sock`; the bytes sent.
function protocol.send_frame(sock, fin, opcode, payload, max_payload_len, masking)
    if payload == nil then
        payload = ""
    elseif type(payload) ~= "string" then
        payload = tostring(payload)
    end
    local length = #payload
    if length > max_payload_len then
        return nil, "payload too big"
    end
    if band(opcode, 0x8) ~= 0 then
        if length > 125 then
            return nil, "too much payload for control frame"
        end
        if not fin then
            return nil, "fragmented control frame"
        end
    end
    local frame, err = build_frame(fin, opcode, length, payload, masking)
    if not frame then
        return nil, "failed to build frame: " .. err
    end
    local bytes
    bytes, err = sock:send(frame)
    if not bytes then
        return nil, "failed to send frame: " .. err
    end
    return bytes
end

return protocol
