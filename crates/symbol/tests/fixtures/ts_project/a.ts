export function greet(name: string): string {
    return `Hello, ${name}!`;
}

export interface Greeter {
    greet(name: string): string;
}

export const DEFAULT_NAME = "World";

export class FriendlyGreeter implements Greeter {
    greet(name: string): string {
        return greet(name);
    }
}
