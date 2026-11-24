// The browser bundle never verifies a TDX quote (a development cluster sends mock ones).
export default {};
export const getCollateralAndVerify = () => {
  throw new Error('TDX verification is not part of this bundle');
};
