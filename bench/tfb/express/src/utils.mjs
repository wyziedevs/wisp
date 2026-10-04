import fjs from 'fast-json-stringify';

export const GREETING = "Hello, World!";

export const jsonSerializer = fjs({
  type: 'object',
  properties: {
    message: {
      type: 'string',
      format: 'unsafe',
    }
  }
});
