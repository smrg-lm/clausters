// Numbers as the reference client prints them.
//
// Every record prints one line and the two clients print the same one, so a
// number in it has to be rounded by one rule. The reference client's is
// Python's own: `f"{x:.1f}"` and `f"{x:g}"` round the **exact** binary value
// of the float to the digits asked for, and take an exact tie to the **even**
// digit. JavaScript's `toFixed` and `toPrecision` round the same exact value
// and take a tie to the **larger** one -- so `4.25` printed `4.3` here and
// `4.2` there, and `100000.5` printed `100001` against `100000`. They differ
// only where a value is exactly halfway, which a float off the wire seldom is
// and a test value often is.
//
// `Number(value.toPrecision(6)).toString()` was also not `%g` past the ties:
// it never writes an exponent where `%g` does (`1e+06`, `1e-05`).
//
// So the rounding is done here, on the float's own bits, with integers.
// Scaling by a power of ten in floating point first would be the obvious
// shortcut and is wrong: `0.05 * 10` is `0.5` exactly, which makes `0.05` look
// like a tie it is not.
//
// Not a rule of the shared core: it is presentation, the same line from one
// description in two languages, and the other language's half is a format
// string.

/**
 * `round(|value| * 10^power)`, exactly, an exact tie going to the even
 * integer. `value` is finite.
 */
function scaled(value: number, power: number): bigint {
    const view = new DataView(new ArrayBuffer(8));
    view.setFloat64(0, Math.abs(value));
    const bits = view.getBigUint64(0);
    const biased = Number((bits >> 52n) & 0x7ffn);
    const fraction = bits & 0xfffffffffffffn;
    // |value| = mantissa * 2^exponent, with nothing lost.
    const mantissa = biased === 0 ? fraction : fraction | (1n << 52n);
    const exponent = (biased === 0 ? 1 : biased) - 1075;
    let numerator = mantissa;
    let denominator = 1n;
    if (exponent >= 0) numerator <<= BigInt(exponent);
    else denominator <<= BigInt(-exponent);
    if (power >= 0) numerator *= 10n ** BigInt(power);
    else denominator *= 10n ** BigInt(-power);
    const quotient = numerator / denominator;
    const twice = 2n * (numerator % denominator);
    const up = twice > denominator || (twice === denominator && (quotient & 1n) === 1n);
    return up ? quotient + 1n : quotient;
}

/** What is not a finite number, as Python spells it; `null` for one that is. */
function unreal(value: number): string | null {
    if (Number.isNaN(value)) return "nan";
    if (!Number.isFinite(value)) return value < 0 ? "-inf" : "inf";
    return null;
}

/** The sign Python writes: a negative zero keeps it, and so does `-0.04` at one decimal. */
function signOf(value: number): string {
    return value < 0 || Object.is(value, -0) ? "-" : "";
}

/**
 * `value` with `digits` decimals, as Python's `f"{value:.{digits}f}"` prints
 * it.
 *
 * @param value - the number to print.
 * @param digits - how many decimals, zero or more.
 */
export function formatFixed(value: number, digits: number): string {
    const other = unreal(value);
    if (other !== null) return other;
    const text = scaled(value, digits).toString().padStart(digits + 1, "0");
    const point = text.length - digits;
    const decimals = digits > 0 ? `.${text.slice(point)}` : "";
    return `${signOf(value)}${text.slice(0, point)}${decimals}`;
}

/**
 * `value` to `precision` significant digits, as Python's `f"{value:g}"`
 * prints it: trailing zeros dropped, and an exponent where the decimal
 * exponent is below -4 or not below the precision (`1e-05`, `1e+06`).
 *
 * @param value - the number to print.
 * @param precision - how many significant digits; `%g`'s own is six.
 */
export function formatGeneral(value: number, precision = 6): string {
    const other = unreal(value);
    if (other !== null) return other;
    const sign = signOf(value);
    if (value === 0) return `${sign}0`;
    // The decimal exponent is the **rounded** value's, so it is settled by
    // the rounding itself: an estimate, moved one step either way until the
    // digits it gives are `precision` of them. `999999.5` is `1e+06`.
    const top = 10n ** BigInt(precision);
    const bottom = top / 10n;
    let exponent = Math.floor(Math.log10(Math.abs(value)));
    let digits = scaled(value, precision - 1 - exponent);
    while (digits >= top || digits < bottom) {
        exponent += digits >= top ? 1 : -1;
        digits = scaled(value, precision - 1 - exponent);
    }
    const text = digits.toString();
    const trimmed = (body: string): string =>
        body.includes(".") ? body.replace(/0+$/, "").replace(/\.$/, "") : body;
    if (exponent < -4 || exponent >= precision) {
        const mantissa = trimmed(`${text[0]}.${text.slice(1)}`);
        const magnitude = Math.abs(exponent).toString().padStart(2, "0");
        return `${sign}${mantissa}e${exponent < 0 ? "-" : "+"}${magnitude}`;
    }
    const body = exponent >= 0
        ? `${text.slice(0, exponent + 1)}.${text.slice(exponent + 1)}`
        : `0.${"0".repeat(-exponent - 1)}${text}`;
    return `${sign}${trimmed(body)}`;
}
