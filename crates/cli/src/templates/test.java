import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;

import PARSER_NS_CLEANED.jtreesitter.LOWER_PARSER_NAME.PARSER_CLASS_NAME;
import io.github.treesitter.jtreesitter.Language;
import org.junit.jupiter.api.Test;

public class PARSER_CLASS_NAMETest {

    @Test
    public void testCanLoadLanguage() {
        assertDoesNotThrow(() -> new Language(PARSER_CLASS_NAME.language()));
    }
}
