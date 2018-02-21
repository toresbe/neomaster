CC=gcc
CXX=g++
RM=rm -f
CPPFLAGS=-g -Wall -Wextra $(shell pkg-config --cflags libusb-1.0)
LDFLAGS=-g 
LDLIBS=$(shell pkg-config --libs libusb-1.0) -lboost_iostreams

PROG=neomaster
SRCS=buttons.cpp neomaster.cpp
OBJS=$(subst .cpp,.o,$(SRCS))

all:		$(PROG)

neomaster:	$(OBJS)
		$(CXX) $(LDFLAGS) -o neomaster $(OBJS) $(LDLIBS) 

clean:
	    $(RM) $(OBJS)

distclean: clean
	    $(RM) neomaster
