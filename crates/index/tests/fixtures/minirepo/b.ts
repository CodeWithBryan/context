export class Counter {
    private n = 0;
    increment(): number {
        this.n += 1;
        return this.n;
    }
}
