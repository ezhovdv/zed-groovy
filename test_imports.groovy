import groovy.transform.Canonical
import groovy.json.JsonSlurper
import com.fasterxml.jackson.databind.ObjectMapper
import com.fasterxml.jackson.annotation.JsonProperty

@Canonical
class TestClass {
    String name
    int age
}

def json = new JsonSlurper().parseText('{"name": "test", "age": 42}')
def mapper = new ObjectMapper()
