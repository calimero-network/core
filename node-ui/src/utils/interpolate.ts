export function interpolate(
  template: string,
  variables: Record<string, string>,
): string {
  return template.replace(
    /{{(.*?)}}/g,
    (_, key) => variables[key.trim()] || '',
  );
}
