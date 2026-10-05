import { describe, expect, it } from 'vitest'
import { areaPath, linePath, runTime, scaleOf } from '../presentation'
import { formatters, nodeOf } from '@/lib/format'

describe('traffic figures', () => {
  const format = formatters('en')

  it('reads latency in milliseconds below a second', () => {
    expect(format.seconds(0.25)).toBe('250 ms')
    expect(format.seconds(1.5)).toBe('1.5 s')
    expect(format.seconds(null)).toBe('—')
  })

  it('reads sizes in binary units', () => {
    expect(format.bytes(512)).toBe('512 B')
    expect(format.bytes(1_572_864)).toBe('1.5 MiB')
  })

  it('rounds counts and keeps two decimals of rates', () => {
    expect(format.count(119.6)).toBe('120')
    expect(format.rate(0.125)).toBe('0.13')
    expect(format.percent(0.125)).toBe('12.5%')
  })
})

describe('upstream nodes', () => {
  it('read as socket addresses', () => {
    expect(nodeOf({ address: '10.0.0.7', port: 8080 })).toBe('10.0.0.7:8080')
    expect(nodeOf({ address: '2001:db8::7', port: 443 })).toBe('[2001:db8::7]:443')
  })
})

describe('traffic charts', () => {
  it('scale every series to the largest finite value', () => {
    expect(scaleOf([[1, null, 4], [2]])).toBe(4)
    expect(scaleOf([[null], []])).toBe(1)
  })

  it('draw lines through the points and break at missing ones', () => {
    expect(linePath([0, 2, null, 4], 4, 30, 10)).toBe('M0.00,10.00L10.00,5.00M30.00,0.00')
  })

  it('fill the area under each unbroken run', () => {
    expect(areaPath([0, 4, null, 4], 4, 30, 10)).toBe('M0.00,10L0.00,10.00L10.00,0.00L10.00,10Z')
  })
})

describe('runTime', () => {
  it('reads Lua run times down to microseconds', () => {
    expect(runTime(0.000_42)).toBe('420 µs')
    expect(runTime(0.0031)).toBe('3.10 ms')
    expect(runTime(0.025)).toBe('25.0 ms')
    expect(runTime(1.5)).toBe('1.50 s')
    expect(runTime(null)).toBe('—')
  })
})
