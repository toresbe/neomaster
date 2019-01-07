CC=cc
CXX=g++
RM=rm -f
#CPPFLAGS=-g -fPIC -std=gnu++11 -Wall -Wextra -I. $(shell pkg-config --cflags libusb-1.0 sdl2 SDL2_ttf SDL2_image)
CPPFLAGS=-g -fPIC -std=gnu++11 -Wall -Wextra -I. -I libpc2 $(shell pkg-config --cflags libusb-1.0)
LDFLAGS=-g 
#LDLIBS=/usr/lib/x86_64-linux-gnu/libboost_log.a $(shell pkg-config --libs libusb-1.0 sdl2 SDL2_ttf SDL2_image) -lboost_iostreams -lboost_thread -lboost_system -lpthread -lboost_log
LDLIBS=/usr/lib/i386-linux-gnu/libboost_log.a $(shell pkg-config --libs libusb-1.0) -lboost_iostreams -lboost_thread -lboost_system -lpthread -lboost_log

PROG=neomaster
#SRCS=$(wildcard modules/*.cpp) $(wildcard ui/*.cpp)  neomaster.cpp
SRCS=core.cpp
OBJS=$(subst .cpp,.o,$(SRCS)) 

all:		$(PROG)

libpc2/libpc2.a:
		$(MAKE) -C libpc2/

neomaster:	$(OBJS) libpc2/libpc2.a
		$(CXX) $(LDFLAGS) -o neomaster $(OBJS) libpc2/libpc2.a $(LDLIBS) 

clean:
	    $(RM) $(OBJS)
	    $(MAKE) -C libpc2/ clean

distclean: clean
	    $(RM) neomaster
	    $(MAKE) -C libpc2/ distclean
