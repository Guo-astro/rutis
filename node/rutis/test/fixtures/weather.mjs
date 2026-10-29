// A plugin that uses a service of the test and provides one, with a
// required config.
import { definePlugin } from '../../src/index.mjs'

class Weather {
  constructor(llm, city) { this.llm = llm; this.city = city }
  async today() { return `${await this.llm.ask(this.city)} in ${this.city}` }
  forecast(days) { return days.map(day => `${day}: fine`) }
}

export default definePlugin({
  name: 'weather',
  inject: ['llm'],
  config: { type: 'object', properties: { city: { type: 'string' }, unit: { type: 'string', default: 'C' } }, required: ['city'] },
  provides: { weather: { today: 'async', forecast: 'sync', unit: 'sync' } },
  apply(ctx, config) {
    const weather = new Weather(ctx.use('llm'), config.city)
    weather.unit = () => config.unit
    ctx.provide('weather', weather)
  },
})
