// The names that moved to `arcships.rutis.advanced` and
// `arcships.rutis.generated` in 0.19 (#692). They still work, as the same
// classes and objects, with a warning that names the new place (the IDE
// replaces them in one step); they are removed when the plugin API version
// changes.
@file:Suppress("unused")

package arcships.rutis

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.Scope.", ReplaceWith("Scope", "arcships.rutis.advanced.Scope"))
typealias Scope = arcships.rutis.advanced.Scope

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.Listener.", ReplaceWith("Listener", "arcships.rutis.advanced.Listener"))
typealias Listener = arcships.rutis.advanced.Listener

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.Next.", ReplaceWith("Next", "arcships.rutis.advanced.Next"))
typealias Next = arcships.rutis.advanced.Next

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.Emitter.", ReplaceWith("Emitter", "arcships.rutis.advanced.Emitter"))
typealias Emitter = arcships.rutis.advanced.Emitter

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.RemoteFunction.", ReplaceWith("RemoteFunction", "arcships.rutis.advanced.RemoteFunction"))
typealias RemoteFunction = arcships.rutis.advanced.RemoteFunction

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.RemoteObject.", ReplaceWith("RemoteObject", "arcships.rutis.advanced.RemoteObject"))
typealias RemoteObject = arcships.rutis.advanced.RemoteObject

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.Kind.", ReplaceWith("Kind", "arcships.rutis.advanced.Kind"))
typealias Kind = arcships.rutis.advanced.Kind

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.ServiceShape.", ReplaceWith("ServiceShape", "arcships.rutis.advanced.ServiceShape"))
typealias ServiceShape = arcships.rutis.advanced.ServiceShape

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.RutisFormat.", ReplaceWith("RutisFormat", "arcships.rutis.advanced.RutisFormat"))
typealias RutisFormat = arcships.rutis.advanced.RutisFormat

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.AnySerializer.", ReplaceWith("AnySerializer", "arcships.rutis.advanced.AnySerializer"))
typealias AnySerializer = arcships.rutis.advanced.AnySerializer

@Deprecated("Moved in 0.19: use arcships.rutis.advanced.JdkSerializers.", ReplaceWith("JdkSerializers", "arcships.rutis.advanced.JdkSerializers"))
typealias JdkSerializers = arcships.rutis.advanced.JdkSerializers

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ServiceTable.", ReplaceWith("ServiceTable<T>", "arcships.rutis.generated.ServiceTable"))
typealias ServiceTable<T> = arcships.rutis.generated.ServiceTable<T>

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ServiceHandle.", ReplaceWith("ServiceHandle", "arcships.rutis.generated.ServiceHandle"))
typealias ServiceHandle = arcships.rutis.generated.ServiceHandle

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ObjectProxy.", ReplaceWith("ObjectProxy", "arcships.rutis.generated.ObjectProxy"))
typealias ObjectProxy = arcships.rutis.generated.ObjectProxy

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ClientFactory.", ReplaceWith("ClientFactory<I>", "arcships.rutis.generated.ClientFactory"))
typealias ClientFactory<I> = arcships.rutis.generated.ClientFactory<I>

@Deprecated("Moved in 0.19: use arcships.rutis.generated.Tables.", ReplaceWith("Tables", "arcships.rutis.generated.Tables"))
typealias Tables = arcships.rutis.generated.Tables

@Deprecated("Moved in 0.19: use arcships.rutis.generated.Generated.", ReplaceWith("Generated", "arcships.rutis.generated.Generated"))
typealias Generated = arcships.rutis.generated.Generated

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ValueEncoder.", ReplaceWith("ValueEncoder", "arcships.rutis.generated.ValueEncoder"))
typealias ValueEncoder = arcships.rutis.generated.ValueEncoder

@Deprecated("Moved in 0.19: use arcships.rutis.generated.ValueDecoder.", ReplaceWith("ValueDecoder", "arcships.rutis.generated.ValueDecoder"))
typealias ValueDecoder = arcships.rutis.generated.ValueDecoder
