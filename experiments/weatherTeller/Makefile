CC      = ../../target/release/ccc
CFLAGS  = -std=c11 -Wall -Wextra -Wpedantic -O2
TARGET  = weather

.PHONY: all clean

all: $(TARGET)

$(TARGET): weather.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(TARGET)
