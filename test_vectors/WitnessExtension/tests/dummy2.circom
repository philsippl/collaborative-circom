pragma circom 2.0.0;

template Example () {
    signal input a[3];
    signal output c[3];
    
    c[1] <-- (a[1]==0) ? 1 : 0;
}

component main { public [ a ] } = Example();