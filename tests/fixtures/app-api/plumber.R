
library(plumber)

#* @get /random_numbers
#* @param maxn

function(maxn) {
  maxn <- as.numeric(maxn)
  runif(1, min = 0, max = maxn)
}
