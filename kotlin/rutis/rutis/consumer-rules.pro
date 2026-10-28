# rutis: kept in applications that shrink (R8), shipped in the AAR.

# The method tables and proxies the rutis-ksp processor generates are found
# by name at run time (arcships.rutis.generated.Tables, as Room and Moshi do): keep
# them, their constructors and the names of the classes they belong to.
-keep class **_RutisService { <init>(); }
-keep class **_RutisClient { <init>(); }
-keepnames @arcships.rutis.RutisService class *
-keepnames @arcships.rutis.RutisClient class *

# An exception crosses with its `code` and `data` properties, read through
# their getters.
-keepclassmembers class * extends java.lang.Throwable {
    public java.lang.String getCode();
    public *** getData();
}

# A suspend function passed where any value is expected is told by its marker.
-keep interface kotlin.coroutines.jvm.internal.SuspendFunction
