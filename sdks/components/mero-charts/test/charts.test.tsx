import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Gauge, TimeSeries, TimeSeriesWithTable, VectorTableRaw, calculateLast, calculateMax, calculateMean } from '../src';

describe('mero-charts', () => {
  it('exports the chart components', () => {
    for (const component of [Gauge, TimeSeries, TimeSeriesWithTable, VectorTableRaw]) {
      expect(component).toBeTypeOf('function');
    }
  });

  it('reduces a series', () => {
    const series = {
      name: 'cpu',
      data: [
        { t: 1, y: 1 },
        { t: 2, y: 5 },
        { t: 3, y: 3 },
      ],
    };
    expect(calculateMean(series)).toBe(3);
    expect(calculateMax(series)).toBe(5);
    expect(calculateLast(series)).toBe(3);
  });

  it('renders a Gauge with its value', () => {
    const html = renderToStaticMarkup(<Gauge value={42} decimals={1} />);
    expect(html).toContain('<svg');
    expect(html).toContain('>42.0</text>');
  });
});
