function isValidIdentifier(identifier) {
    try {
        // identifier may be an invalid variable name (e.g. 'a-b')
        eval("var " + identifier + ";");
        return true
      } catch (e) {
          return false;
      }
}

/**
 * Declares all globals with a var and assigns the global object. Thus you're able to
 * override globals without changing the global object itself.
 *
 * Returns something like
 * "var console = global.console; var process = global.process; ..."
 *
 * @return {String}
 */
function getImportGlobalsSrc(ignore) {
    var key,
        src = "",
        globalObj = typeof global === "undefined"? window: global;

    ignore = ignore || [];
    // global itself can't be overridden because it's the only reference to our real global objects
    ignore.push("global", "globalThis");
    ignore.push("GLOBAL", "root");
    ignore.push("undefined");
    ignore.push("eval");
    // ignore 'module', 'exports' and 'require' on the global scope, because otherwise our code would
    // shadow the module-internal variables
    // @see https://github.com/jhnns/rewire-webpack/pull/6
    ignore.push("module", "exports", "require");

    for (key in Object.getOwnPropertyDescriptors(globalObj)) { /* jshint forin: false */
        if (ignore.indexOf(key) !== -1) {
            continue;
        }

        if (isValidIdentifier(key)) {
            src += "var " + key + " = global." + key + "; ";
        }
    }

    return src;
}

module.exports = getImportGlobalsSrc;
